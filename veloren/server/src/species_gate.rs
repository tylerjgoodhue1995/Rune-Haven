//! Only Human characters are free. Every other humanoid species needs a character NFT in the player's wallet,
//! which the Rune Haven site API confirms on Solana.

use common::comp::{Body, humanoid::Species};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::login_provider::is_admin_wallet;

const CACHE_TTL: Duration = Duration::from_secs(30);

struct Cache(Mutex<HashMap<String, (Instant, Vec<String>)>>);

fn cache() -> &'static Cache { 
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Cache(Mutex::new(HashMap::new())))
}

fn owned_species(wallet: &str) -> Result<Vec<String>, String> {
    if let Some((at, species)) = cache().0.lock().unwrap_or_else(|p| p.into_inner()).get(wallet)
        && at.elapsed() < CACHE_TTL
    {
        return Ok(species.clone());
    }
    let (Ok(url), Ok(key)) = (
        std::env::var("VELOREN_SPECIES_GATE_URL"),
        std::env::var("VELOREN_SPECIES_GATE_KEY"),
    ) else {
        return Err("Character ownership checks are not configured on this server.".to_string());
    };
    let unavailable = || "Could not verify your character NFTs right now. Try again shortly.".to_string();
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(|_| unavailable())?
        .get(format!("{}/api/v1/game/entitlements/{wallet}", url.trim_end_matches('/')))
        .header("x-game-key", key)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|_| unavailable())?;
    let body: serde_json::Value = response.json().map_err(|_| unavailable())?;
    let species: Vec<String> = body
        .get("species")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .ok_or_else(unavailable)?;
    cache()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(wallet.to_owned(), (Instant::now(), species.clone()));
    Ok(species)
}

/// `wallet` is the player's linked wallet, if any.
pub fn check(wallet: Option<&str>, body: &Body) -> Result<(), String> {
    let Body::Humanoid(humanoid) = body else { return Ok(()) };
    if humanoid.species == Species::Human {
        return Ok(());
    }
    let name = format!("{:?}", humanoid.species);
    let locked = || {
        format!(
            "{name} characters are NFTs. Buy one on the Rune Haven marketplace, or choose Human."
        )
    };
    let wallet = wallet.ok_or_else(locked)?;
    if is_admin_wallet(wallet) || owned_species(wallet)?.iter().any(|owned| owned == &name) {
        Ok(())
    } else {
        Err(locked())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::comp::humanoid;

    fn body(species: Species) -> Body {
        Body::Humanoid(humanoid::Body {
            species,
            body_type: humanoid::BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
            height_scale: 128,
        })
    }

    #[test]
    fn human_is_always_allowed() {
        assert!(check(None, &body(Species::Human)).is_ok());
    }

    #[test]
    fn other_species_are_locked_without_a_wallet() {
        for species in [Species::Orc, Species::Elf, Species::Dwarf, Species::Danari, Species::Draugr] {
            assert!(check(None, &body(species)).is_err());
        }
    }

    #[test]
    fn admin_wallet_bypasses_the_gate() {
        assert!(check(Some("EiL5hGfzLAyCah2GMrxFz47HPLwgK6CQtS1CL1gWxQF8"), &body(Species::Orc)).is_ok());
    }
}
