//! The game's light of a city for the views (stage 66): the time-of-day keys
//! of the city's `local` stream, read once per game and city, turned into the
//! renderer's [`GameLook`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::time_of_day::{self, CityLight};

use crate::render::GameLook;

type Cache = HashMap<(PathBuf, String), Option<Arc<CityLight>>>;

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The keys of a city (`cuba`, `queens`, `miami`); `None` when the game has
/// none for it. Read on first use (tens of milliseconds), then shared.
pub fn city(game: &GameInstance, city: &str) -> Option<Arc<CityLight>> {
    let key = (game.root().to_path_buf(), city.to_owned());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    cache
        .entry(key)
        .or_insert_with(|| {
            CityLight::read(game, city)
                .ok()
                .filter(|l| !l.outdoor.is_empty())
                .map(Arc::new)
        })
        .clone()
}

/// The look of the city of `stream` at `hours` (from midnight), if shading
/// as in the game is on and the stream is in a city.
pub fn look(game: &GameInstance, stream: &str, hours: f32) -> Option<GameLook> {
    if !crate::render::game_look() {
        return None;
    }
    let name = time_of_day::city_of(stream)?;
    let light = city(game, name)?;
    Some(GameLook::from_look(&light.at(seconds(hours))))
}

/// The look of the model and character viewers (no city around).
pub fn studio() -> Option<GameLook> {
    crate::render::game_look().then(GameLook::studio)
}

/// Hours from midnight as whole seconds of a day.
pub fn seconds(hours: f32) -> u32 {
    (hours.rem_euclid(24.0) * 3600.0).round() as u32 % time_of_day::DAY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_wrap_into_a_day() {
        assert_eq!(seconds(12.5), 45_000);
        assert_eq!(seconds(-1.0), 23 * 3600);
        assert_eq!(seconds(24.0), 0);
    }
}
