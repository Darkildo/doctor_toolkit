//! When the sound tracks of a cut-scene start, how loud they play, and
//! their samples.
//!
//! The speech and effects track (`aud_igc06_carminestruce.exa.snu`, six
//! channels) starts with the cut-scene. The music (`mus_igc06_01.exa.snu`)
//! is started by the audio scripts of the game: a text command of the
//! cut-scene sends a message (`music_igc06_01`), and a handler of a `lua`
//! script of `gf2/globals_global` plays the stream:
//!
//! ```lua
//! music_igc06_01 = function()
//!   currentIGCMusic = PlayEarsAudioStream("mus_IGC06_01.stx", 0.6)
//! end,
//! ```
//!
//! The second argument is a volume multiplier (1 when absent). A handler can
//! also choose a cue of the music system (`sCueCharacter = "igc03"`) whose
//! table entry names the stream and its volume
//! (`igc03 = { "mus_IGC03", false, 1, 0, nil, 1 }`). The tool reads these
//! handlers from the decompiled scripts ([`stream_cues`]) and places each
//! track at the first message of the cut-scene that starts it ([`plan`]).

use serde::Serialize;

use crate::Result;
use crate::resource::audio::Sample;
use crate::resource::lua::{self, Chunk, TYPE_LUA};
use crate::resource::trinity::Command;
use crate::resource::video::input::Pcm;

use super::{AudioTrack, Shot, Source};

/// The stream with the audio scripts.
pub const SCRIPT_STREAM: &str = "gf2/globals_global";

/// A message handler of an audio script that plays a streamed sound.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamCue {
    /// The message (`music_igc06_01`), lower case.
    pub message: String,
    /// The stream played, lower case without extension (`mus_igc06_01`).
    pub stream: String,
    /// Volume multiplier.
    pub volume: f32,
}

/// Where a sound track plays in its cut-scene.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    /// Seconds from the start of the cut-scene; `None` when the track is
    /// started by a message the cut-scene does not send.
    pub start: Option<f32>,
    /// Volume multiplier.
    pub volume: f32,
    /// The message that starts it (`None` for the cut-scene's own track).
    pub message: Option<String>,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            start: Some(0.0),
            volume: 1.0,
            message: None,
        }
    }
}

/// Data of the `lua` resources of [`SCRIPT_STREAM`] that start streamed
/// sounds, as the workspace has them.
pub fn sound_scripts(source: &Source<'_>) -> Result<Vec<Vec<u8>>> {
    let Some(stream) = source.game.find_stream(SCRIPT_STREAM) else {
        return Ok(Vec::new());
    };
    let file = source.game.open_stream(stream)?;
    let mut out = Vec::new();
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_LUA {
            continue;
        }
        let data = source.resource(stream, &file, index)?;
        if contains(&data, b"PlayEarsAudioStream") || contains(&data, b"sCueCharacter") {
            out.push(data);
        }
    }
    Ok(out)
}

fn contains(data: &[u8], what: &[u8]) -> bool {
    data.windows(what.len()).any(|w| w == what)
}

/// The stream cues of compiled scripts (see [`parse_cues`]); scripts that do
/// not parse are skipped.
pub fn stream_cues(scripts: &[Vec<u8>]) -> Vec<StreamCue> {
    let sources: Vec<String> = scripts
        .iter()
        .filter_map(|data| Chunk::parse(data).ok())
        .map(|chunk| lua::decompile(&chunk).source)
        .collect();
    parse_cues(&sources)
}

/// The message handlers of Lua sources that start a streamed sound, directly
/// (`PlayEarsAudioStream("name.stx", volume)`) or by a cue of the music
/// system (`sCueCharacter = "cue"` with a table entry
/// `cue = { "name", …, volume }` in any of the sources).
pub fn parse_cues(sources: &[String]) -> Vec<StreamCue> {
    // Cue table entries: cue → (stream, volume).
    let mut table: Vec<(String, String, f32)> = Vec::new();
    for source in sources {
        for line in source.lines() {
            if let Some((name, rest)) = assignment(line)
                && let Some(body) = rest.strip_prefix('{')
                && let Some(body) = body.trim_end_matches(',').trim().strip_suffix('}')
            {
                let fields: Vec<&str> = body.split(',').map(str::trim).collect();
                if let Some(stream) = fields.first().and_then(|f| string_literal(f))
                    && fields.len() > 1
                {
                    let volume = fields.last().and_then(|f| f.parse().ok()).unwrap_or(1.0);
                    table.push((name.to_ascii_lowercase(), stem(stream), volume));
                }
            }
        }
    }
    let mut out = Vec::new();
    for source in sources {
        let mut handler: Option<String> = None;
        for line in source.lines() {
            let text = line.trim();
            if let Some((name, rest)) = assignment(line)
                && (rest.is_empty() || rest.starts_with("function("))
            {
                handler = Some(name.to_ascii_lowercase());
                continue;
            }
            let Some(message) = &handler else { continue };
            if let Some(at) = text.find("PlayEarsAudioStream(") {
                let args = &text[at + "PlayEarsAudioStream(".len()..];
                let args = args.split(')').next().unwrap_or_default();
                let mut args = args.split(',').map(str::trim);
                if let Some(stream) = args.next().and_then(string_literal) {
                    let volume = args.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
                    push(&mut out, message, stem(stream), volume);
                }
            } else if let Some(cue) = text
                .strip_prefix("sCueCharacter = ")
                .and_then(string_literal)
                && let Some((_, stream, volume)) = table
                    .iter()
                    .find(|(name, ..)| name.eq_ignore_ascii_case(cue))
            {
                push(&mut out, message, stream.clone(), *volume);
            }
        }
    }
    out
}

fn push(out: &mut Vec<StreamCue>, message: &str, stream: String, volume: f32) {
    let cue = StreamCue {
        message: message.to_owned(),
        stream,
        volume,
    };
    if !out.contains(&cue) {
        out.push(cue);
    }
}

/// `name = rest` with `name` an identifier: the name and the trimmed rest.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = line.trim().split_once(" =")?;
    let ident = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (ident && (rest.is_empty() || rest.starts_with(' '))).then(|| (name, rest.trim()))
}

fn string_literal(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
}

/// File name without directories and extensions, lower case:
/// `audiostreams\igc06_01\mus_igc06_01.exa.snu` and `mus_IGC06_01.stx` give
/// `mus_igc06_01`.
pub fn stem(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.split('.').next().unwrap_or(file).to_ascii_lowercase()
}

/// The messages the text commands of the shots send (kind 1), with their
/// time in seconds from the start of the cut-scene, in playing order.
pub fn messages(shots: &[Shot]) -> Vec<(f32, String)> {
    let mut out = Vec::new();
    for shot in shots {
        let block = shot.block();
        for (command, at) in block.commands.iter().zip(block.times()) {
            if let Command::Script { kind: 1, text, .. } = command {
                out.push((shot.start + at, text.text.clone()));
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Where each track plays: a track no cue plays is the cut-scene's own and
/// starts with it at full volume; a track a cue plays starts at the first
/// message of the cut-scene that cue answers.
pub fn plan(
    audio: &[AudioTrack],
    messages: &[(f32, String)],
    cues: &[StreamCue],
) -> Vec<Placement> {
    audio
        .iter()
        .map(|track| {
            let name = stem(&track.entry);
            let cues: Vec<&StreamCue> = cues.iter().filter(|c| c.stream == name).collect();
            if cues.is_empty() {
                return Placement::default();
            }
            let first = messages.iter().find_map(|(at, m)| {
                cues.iter()
                    .find(|c| c.message.eq_ignore_ascii_case(m))
                    .map(|c| (*at, *c))
            });
            match first {
                Some((at, cue)) => Placement {
                    start: Some(at),
                    volume: cue.volume,
                    message: Some(cue.message.clone()),
                },
                None => Placement {
                    start: None,
                    volume: cues[0].volume,
                    message: Some(cues[0].message.clone()),
                },
            }
        })
        .collect()
}

/// Places the tracks of a cut-scene (sets [`AudioTrack::placement`]).
pub fn place(source: &Source<'_>, shots: &[Shot], audio: &mut [AudioTrack]) -> Result<()> {
    let cues = stream_cues(&sound_scripts(source)?);
    let placements = plan(audio, &messages(shots), &cues);
    for (track, placement) in audio.iter_mut().zip(placements) {
        track.placement = placement;
    }
    Ok(())
}

/// Decodes a streamed sound (`.snu`) to 16-bit PCM.
pub fn decode(snu: &[u8]) -> Result<Pcm> {
    let sample = Sample::parse_snu(snu)?;
    Ok(Pcm {
        channels: sample.header.channels as u16,
        sample_rate: sample.header.sample_rate,
        samples: sample.decode()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"
underscoreSpecialEvents =
  { {
    igc03 = { "mus_IGC03", false, 1, 0, nil, 0.8 },
    reward = { "reward_01", true, 1, 0, nil, 1 } }
}
handlers = {
  music_igc06_01 =
    function()
    debug_screenprint(sShowInfo, "'PLAY IGC06_01' MESSAGE RECEIVED")
    currentIGCMusic = PlayEarsAudioStream("mus_IGC06_01.stx")
  end,
  music_igc_16 = function()
    currentIGCMusic = PlayEarsAudioStream("mus_igc16.stx", 0.6)
  end,
  music_igc_03 =
    function()
    sCueCharacter = "igc03"
    SendMessage("music_special_event")
  end,
  music_reward = function()
    if sCurrentCueCharacter ~= "reward" then
      sCueCharacter = "nothing"
    end
  end,
}
"#;

    #[test]
    fn finds_handlers_that_play_streams() {
        let cues = parse_cues(&[SCRIPT.to_owned()]);
        let cue = |m: &str, s: &str, v: f32| StreamCue {
            message: m.into(),
            stream: s.into(),
            volume: v,
        };
        assert_eq!(
            cues,
            [
                cue("music_igc06_01", "mus_igc06_01", 1.0),
                cue("music_igc_16", "mus_igc16", 0.6),
                cue("music_igc_03", "mus_igc03", 0.8),
            ]
        );
    }

    fn track(entry: &str) -> AudioTrack {
        AudioTrack {
            entry: entry.into(),
            duration_ms: 1000,
            channels: 2,
            tob: None,
            subtitles: Vec::new(),
            placement: Placement::default(),
        }
    }

    #[test]
    fn places_tracks_at_their_messages() {
        let cues = parse_cues(&[SCRIPT.to_owned()]);
        let audio = [
            track(r"audiostreams\igc06_ca\aud_igc06_carminestruce.exa.snu"),
            track(r"audiostreams\igc06_01\mus_igc06_01.exa.snu"),
            track(r"audiostreams\igc16\mus_igc16.exa.snu"),
        ];
        let messages = [
            (0.5, "aud_init".to_owned()),
            (10.0, "MUSIC_IGC06_01".to_owned()),
            (12.0, "music_igc06_01".to_owned()),
        ];
        let plan = plan(&audio, &messages, &cues);
        assert_eq!(plan[0], Placement::default());
        assert_eq!(plan[1].start, Some(10.0));
        assert_eq!(plan[1].message.as_deref(), Some("music_igc06_01"));
        assert_eq!(plan[2].start, None, "music_igc_16 is not sent");
        assert_eq!(plan[2].volume, 0.6);
    }

    #[test]
    fn stems() {
        assert_eq!(stem(r"audiostreams\a\MUS_X.exa.snu"), "mus_x");
        assert_eq!(stem("mus_IGC02_NewYearsEve.stx"), "mus_igc02_newyearseve");
    }
}
