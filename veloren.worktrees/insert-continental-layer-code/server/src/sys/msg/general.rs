use crate::{client::Client, property::PropertyRuntime};
use common::{
    comp::{ChatMode, ChatType, Content, Group, Player},
    event::{self, EmitExt},
    event_emitters,
    resources::ProgramTime,
    uid::Uid,
};
use common_ecs::{Job, Origin, Phase, System};
use common_net::msg::{ClientGeneral, PropertyPurchaseState, ServerGeneral};
use specs::{Entities, Join, LendJoin, Read, ReadStorage, Write, WriteStorage};
use tracing::{debug, error, warn};

event_emitters! {
    struct Events[Emitters] {
        command: event::CommandEvent,
        client_disconnect: event::ClientDisconnectEvent,
        chat: event::ChatEvent,

        #[cfg(feature = "plugins")]
        plugins: event::RequestPluginsEvent,
    }
}

impl Sys {
    fn handle_general_msg(
        emitters: &mut Emitters,
        entity: specs::Entity,
        client: &Client,
        player: Option<&Player>,
        property_runtime: &mut PropertyRuntime,
        uids: &ReadStorage<'_, Uid>,
        chat_modes: &ReadStorage<'_, ChatMode>,
        groups: &ReadStorage<'_, Group>,
        msg: ClientGeneral,
    ) -> Result<(), crate::error::Error> {
        match msg {
            ClientGeneral::RequestPropertyParcels => {
                let parcels = player
                    .map(|player| {
                        property_runtime.parcel_infos_for_player(&player.uuid().to_string())
                    })
                    .unwrap_or_default();
                client.send(ServerGeneral::PropertyParcels(
                    parcels,
                ))?;
            },
            ClientGeneral::RequestPropertyPurchase { parcel_id } => {
                let Some(player) = player else {
                    warn!(?entity, "property purchase requested without player data");
                    return Ok(());
                };

                let player_id = player.uuid().to_string();
                let parcel = property_runtime
                    .parcel_infos_for_player(&player_id)
                    .into_iter()
                    .find(|parcel| parcel.id == parcel_id);
                let (state, message) = match parcel {
                    None => (
                        PropertyPurchaseState::Failed,
                        "That property is no longer available.".to_string(),
                    ),
                    Some(parcel) if parcel.is_owned => (
                        PropertyPurchaseState::Owned,
                        "You already own this property.".to_string(),
                    ),
                    Some(parcel) if parcel.placed_buildings >= parcel.max_buildings => (
                        PropertyPurchaseState::Failed,
                        "That property is currently at capacity.".to_string(),
                    ),
                    Some(parcel) if std::env::var("VELOREN_PROPERTY_SANDBOX_PURCHASE")
                        .ok()
                        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")) =>
                    {
                        if property_runtime.sandbox_assign_land(&player_id, &parcel.id) {
                            (
                                PropertyPurchaseState::Owned,
                                "Sandbox purchase complete. This development-only ownership is not a blockchain transaction.".to_string(),
                            )
                        } else {
                            (
                                PropertyPurchaseState::Failed,
                                "Sandbox purchase is unavailable for this property or wallet.".to_string(),
                            )
                        }
                    },
                    Some(_) => (
                        PropertyPurchaseState::Unavailable,
                        "Property purchases are not configured on this server yet. No wallet transaction was created.".to_string(),
                    ),
                };
                client.send(ServerGeneral::PropertyPurchaseResult {
                    parcel_id,
                    state,
                    message,
                })?;
            },
            ClientGeneral::RequestWalletChallenge => {
                let Some(player) = player else {
                    warn!(?entity, "wallet challenge requested without player data");
                    return Ok(());
                };

                let challenge = property_runtime.issue_wallet_challenge(&player.uuid().to_string());
                client.send(ServerGeneral::WalletChallenge { challenge })?;
            },
            ClientGeneral::LinkWallet {
                wallet,
                challenge,
                signature,
            } => {
                let Some(player) = player else {
                    warn!(?entity, "wallet association requested without player data");
                    return Ok(());
                };

                let player_id = player.uuid().to_string();
                let result =
                    property_runtime.link_wallet(&player_id, &wallet, &challenge, &signature);
                client.send(ServerGeneral::WalletLinkResult {
                    success: result.is_ok(),
                    message: result
                        .map(|_| "wallet linked successfully".to_string())
                        .unwrap_or_else(|err| err.to_string()),
                })?;
            },
            ClientGeneral::RequestPropertyPlacement {
                parcel_id,
                land_nft_id,
                building_nft_id,
                x,
                y,
            } => {
                let Some(player) = player else {
                    warn!(?entity, "property placement requested without player data");
                    return Ok(());
                };

                let player_id = player.uuid().to_string();
                let result = property_runtime.authorize_placement(
                    &player_id,
                    &parcel_id,
                    &land_nft_id,
                    &building_nft_id,
                    crate::property::WorldPosition { x, y },
                );
                let building_type = if result.is_ok() {
                    property_runtime.building_type_for_placement(&building_nft_id)
                } else {
                    None
                };
                client.send(ServerGeneral::PropertyPlacementResult {
                    success: result.is_ok(),
                    parcel_id: parcel_id.clone(),
                    x,
                    y,
                    building_type,
                    message: result
                        .map(|_| "placement authorized".to_string())
                        .unwrap_or_else(|err| err.to_string()),
                })?;
            },
            ClientGeneral::ChatMsg(message) => {
                if !client.client_type.can_send_message() {
                    client.send_fallible(ServerGeneral::ChatMsg(
                        ChatType::CommandError
                            .into_msg(Content::localized("command-cannot-send-message-hidden")),
                    ));
                } else if player.is_some() {
                    if let Some(from) = uids.get(entity) {
                        const CHAT_MODE_DEFAULT: &ChatMode = &ChatMode::default();
                        let mode = chat_modes.get(entity).unwrap_or(CHAT_MODE_DEFAULT);
                        // Try sending the chat message
                        match mode.to_msg(*from, message, groups.get(entity).copied()) {
                            Ok(message) => {
                                emitters.emit(event::ChatEvent {
                                    msg: message,
                                    from_client: true,
                                });
                            },
                            Err(error) => {
                                client.send_fallible(ServerGeneral::ChatMsg(
                                    ChatType::CommandError.into_msg(error),
                                ));
                            },
                        }
                    } else {
                        error!("Could not send message. Missing player uid");
                    }
                } else {
                    warn!("Received a chat message from an unregistered client");
                }
            },
            ClientGeneral::Command(name, args) => {
                if player.is_some() {
                    emitters.emit(event::CommandEvent(entity, name, args));
                }
            },
            ClientGeneral::Terminate => {
                debug!(?entity, "Client send message to terminate session");
                emitters.emit(event::ClientDisconnectEvent(
                    entity,
                    common::comp::DisconnectReason::ClientRequested,
                ));
            },
            ClientGeneral::RequestPlugins(plugins) => {
                tracing::info!("Plugin request {plugins:x?}, {}", player.is_some());

                #[cfg(feature = "plugins")]
                emitters.emit(event::RequestPluginsEvent { entity, plugins });
            },
            _ => {
                debug!("Kicking possible misbehaving client due to invalid message request");
                emitters.emit(event::ClientDisconnectEvent(
                    entity,
                    common::comp::DisconnectReason::NetworkError,
                ));
            },
        }
        Ok(())
    }
}

/// This system will handle new messages from clients
#[derive(Default)]
pub struct Sys;
impl<'a> System<'a> for Sys {
    type SystemData = (
        Entities<'a>,
        Events<'a>,
        Read<'a, ProgramTime>,
        ReadStorage<'a, Uid>,
        ReadStorage<'a, ChatMode>,
        ReadStorage<'a, Player>,
        ReadStorage<'a, Group>,
        WriteStorage<'a, Client>,
        Write<'a, PropertyRuntime>,
    );

    const NAME: &'static str = "msg::general";
    const ORIGIN: Origin = Origin::Server;
    const PHASE: Phase = Phase::Create;

    fn run(
        _job: &mut Job<Self>,
        (
            entities,
            events,
            program_time,
            uids,
            chat_modes,
            players,
            groups,
            mut clients,
            mut property_runtime,
        ): Self::SystemData,
    ) {
        let mut emitters = events.get_emitters();

        for (entity, client, player) in (&entities, &mut clients, players.maybe()).join() {
            let res = super::try_recv_all(client, 3, |client, msg| {
                Self::handle_general_msg(
                    &mut emitters,
                    entity,
                    client,
                    player,
                    &mut property_runtime,
                    &uids,
                    &chat_modes,
                    &groups,
                    msg,
                )
            });

            if let Ok(1_u64..=u64::MAX) = res {
                // Update client ping.
                client.last_ping = program_time.0
            }
        }
    }
}
