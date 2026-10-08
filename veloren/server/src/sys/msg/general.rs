use crate::{
    client::Client,
    property::ActivePropertyRuntime,
    vgld::{
        DepositVerifier, SolanaDepositVerifier, VgldConfig, VgldLedger, apply_verified_deposit,
    },
};
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
use std::time::{SystemTime, UNIX_EPOCH};
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
        property_runtime: &mut ActivePropertyRuntime,
        vgld_ledger: &mut VgldLedger,
        vgld_config: &VgldConfig,
        uids: &ReadStorage<'_, Uid>,
        chat_modes: &ReadStorage<'_, ChatMode>,
        groups: &ReadStorage<'_, Group>,
        msg: ClientGeneral,
    ) -> Result<(), crate::error::Error> {
        match msg {
            ClientGeneral::RequestPropertyParcels => {
                property_runtime.reload_parcels();
                let parcels = player
                    .map(|player| {
                        property_runtime.parcel_infos_for_player(&player.uuid().to_string())
                    })
                    .unwrap_or_default();
                client.send(ServerGeneral::PropertyParcels(parcels))?;
            },
            ClientGeneral::RequestPropertyPurchase { parcel_id } => {
                let Some(player) = player else {
                    warn!(?entity, "property purchase requested without player data");
                    return Ok(());
                };

                let player_id = player.uuid().to_string();
                let reservation_price = property_runtime.reserve_purchase(&player_id, &parcel_id);
                if let Ok(price) = reservation_price {
                    if vgld_ledger.balance(&player_id) < price {
                        property_runtime.release_purchase(&parcel_id);
                        client.send(ServerGeneral::PropertyPurchaseResult {
                            parcel_id,
                            state: PropertyPurchaseState::Failed,
                            message: "You do not have enough VGLD for this property.".to_string(),
                        })?;
                        return Ok(());
                    }
                    let test_mode = std::env::var("VELOREN_PROPERTY_VGLD_PURCHASE_TEST_MODE")
                        .ok()
                        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
                    if test_mode {
                        let purchase_nonce = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos();
                        let transaction_id =
                            format!("property-purchase:{player_id}:{parcel_id}:{purchase_nonce}");
                        if let Err(error) = vgld_ledger.debit(&player_id, price, &transaction_id) {
                            property_runtime.release_purchase(&parcel_id);
                            client.send(ServerGeneral::PropertyPurchaseResult {
                                parcel_id,
                                state: PropertyPurchaseState::Failed,
                                message: format!("VGLD purchase debit failed: {error:?}"),
                            })?;
                            return Ok(());
                        }
                        if property_runtime.complete_sandbox_purchase(&player_id, &parcel_id) {
                            client.send(ServerGeneral::PropertyPurchaseResult {
                                parcel_id,
                                state: PropertyPurchaseState::Owned,
                                message: "Test-mode purchase complete. VGLD was debited from the \
                                          server ledger; no blockchain transaction was created."
                                    .to_string(),
                            })?;
                            return Ok(());
                        }
                        let refund_id = format!("{transaction_id}:refund");
                        if let Err(error) = vgld_ledger.credit(&player_id, price, &refund_id) {
                            error!(?error, %player_id, %parcel_id, "Failed to refund unsuccessful property test purchase");
                        }
                        property_runtime.release_purchase(&parcel_id);
                        client.send(ServerGeneral::PropertyPurchaseResult {
                            parcel_id,
                            state: PropertyPurchaseState::Failed,
                            message: "Test-mode purchase could not assign the development land \
                                      NFT; the VGLD debit was refunded."
                                .to_string(),
                        })?;
                        return Ok(());
                    }
                    client.send(ServerGeneral::PropertyPurchaseResult {
                        parcel_id,
                        state: PropertyPurchaseState::Pending,
                        message: "Property reserved. Blockchain settlement is not enabled on this \
                                  server yet; no VGLD was debited."
                            .to_string(),
                    })?;
                    return Ok(());
                }
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
                    Some(parcel)
                        if std::env::var("VELOREN_PROPERTY_SANDBOX_PURCHASE")
                            .ok()
                            .is_some_and(|value| {
                                value == "1" || value.eq_ignore_ascii_case("true")
                            }) =>
                    {
                        if property_runtime.sandbox_assign_land(&player_id, &parcel.id) {
                            (
                                PropertyPurchaseState::Owned,
                                "Sandbox purchase complete. This development-only ownership is \
                                 not a blockchain transaction."
                                    .to_string(),
                            )
                        } else {
                            (
                                PropertyPurchaseState::Failed,
                                "Sandbox purchase is unavailable for this property or wallet."
                                    .to_string(),
                            )
                        }
                    },
                    Some(_) => (
                        PropertyPurchaseState::Unavailable,
                        "Property purchases are not configured on this server yet. No wallet \
                         transaction was created."
                            .to_string(),
                    ),
                };
                client.send(ServerGeneral::PropertyPurchaseResult {
                    parcel_id,
                    state,
                    message,
                })?;
            },
            ClientGeneral::RequestVgldAccount => {
                let Some(player) = player else {
                    warn!(?entity, "VGLD account requested without player data");
                    return Ok(());
                };

                let player_id = player.uuid().to_string();
                let account = property_runtime.account(&player_id);
                let wallet = account.as_ref().map(|account| account.wallet.clone());
                let balance_base_units = account
                    .as_ref()
                    .map(|account| vgld_ledger.balance(&account.player_id))
                    .unwrap_or_default();
                let status = if wallet.is_some() {
                    "Wallet linked".to_string()
                } else {
                    "Wallet not linked".to_string()
                };
                client.send(ServerGeneral::VgldAccount {
                    wallet,
                    balance_base_units,
                    decimals: crate::vgld::VGLD_DECIMALS,
                    status,
                })?;
            },
            ClientGeneral::RequestVgldDeposit { transaction_id } => {
                let Some(player) = player else {
                    warn!(?entity, "VGLD deposit requested without player data");
                    return Ok(());
                };
                let player_id = player.uuid().to_string();
                let Some(account) = property_runtime.account(&player_id) else {
                    client.send(ServerGeneral::VgldAccount {
                        wallet: None,
                        balance_base_units: 0,
                        decimals: crate::vgld::VGLD_DECIMALS,
                        status: "Link a wallet before depositing VGLD".to_string(),
                    })?;
                    return Ok(());
                };
                let Some(mint) = vgld_config.mint_address.as_deref() else {
                    client.send(ServerGeneral::VgldAccount {
                        wallet: Some(account.wallet),
                        balance_base_units: vgld_ledger.balance(&account.player_id),
                        decimals: crate::vgld::VGLD_DECIMALS,
                        status: "VGLD deposits are not configured on this server".to_string(),
                    })?;
                    return Ok(());
                };
                let Some(treasury) = vgld_config.treasury_address.as_deref() else {
                    client.send(ServerGeneral::VgldAccount {
                        wallet: Some(account.wallet),
                        balance_base_units: vgld_ledger.balance(&account.player_id),
                        decimals: crate::vgld::VGLD_DECIMALS,
                        status: "VGLD deposits are not configured on this server".to_string(),
                    })?;
                    return Ok(());
                };
                let verifier = SolanaDepositVerifier::new(&vgld_config.rpc_url);
                let status = match verifier
                    .verify_deposit(&transaction_id, &account.wallet, mint, Some(treasury))
                    .and_then(|deposit| {
                        apply_verified_deposit(
                            vgld_ledger,
                            &account.player_id,
                            &account.wallet,
                            mint,
                            deposit,
                        )
                    }) {
                    Ok(()) => "Deposit verified and credited".to_string(),
                    Err(error) => format!("Deposit rejected: {error:?}"),
                };
                client.send(ServerGeneral::VgldAccount {
                    wallet: Some(account.wallet),
                    balance_base_units: vgld_ledger.balance(&account.player_id),
                    decimals: crate::vgld::VGLD_DECIMALS,
                    status,
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
        Write<'a, ActivePropertyRuntime>,
        Write<'a, VgldLedger>,
        Read<'a, VgldConfig>,
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
            mut vgld_ledger,
            vgld_config,
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
                    &mut vgld_ledger,
                    &vgld_config,
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
