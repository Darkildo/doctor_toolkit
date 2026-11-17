//! Stage 77: the story of the game as its SimGroups hold it
//! (docs/analysis/story.md §3–§6).

use std::collections::{BTreeMap, BTreeSet};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::{ScopeSpec, scope};
use doctor_toolkit::resource::simgroup::Value;

/// The table of missions: 166 checkpoints with numbers of their own in 69
/// chapters, every one in the root stream of a city or in the globals; the
/// objectives all share one title string; `UIJobInfo` has only the kinds
/// of favours.
#[test]
fn checkpoints_chapters_objectives() {
    let Some(dir) = super::game_dir() else { return };
    let game = GameInstance::open(&dir).unwrap();
    let (index, _) = game.build_index(|_| {});
    let locations = scope::groups(&game, &index, &ScopeSpec::Game).unwrap();
    let (loaded, errors) = scope::load(&game, None, &locations);
    assert!(errors.is_empty(), "{:?}", errors.first());

    let (checkpoint, objective, job_info, test) = (
        string_id("Checkpoint"),
        string_id("UIHudObjective"),
        string_id("UIJobInfo"),
        string_id("TestCurrentCheckpoint"),
    );
    let int = |value: &Value| match value {
        Value::Int(n) => Some(*n),
        _ => None,
    };
    let mut numbers = BTreeSet::new();
    let mut chapters = BTreeSet::new();
    let mut streams: BTreeMap<String, usize> = BTreeMap::new();
    let mut checkpoints = 0;
    let mut titles = BTreeSet::new();
    let mut objectives = 0;
    let mut kinds = BTreeSet::new();
    let mut at_least = 0;
    let mut tests = 0;
    for g in &loaded {
        for o in &g.group.objects {
            let Some(own) = g.group.object_packets(o).find(|p| p.class.0 == o.class) else {
                continue;
            };
            let attr = |i: u16| own.attrs.iter().find(|a| a.index == i).map(|a| &a.value);
            if o.class == checkpoint {
                checkpoints += 1;
                numbers.insert(attr(1).and_then(int).expect("a checkpoint has a number"));
                chapters.insert(attr(3).and_then(int).expect("a checkpoint has a chapter"));
                *streams.entry(g.stream.replace('\\', "/")).or_default() += 1;
            } else if o.class == objective {
                objectives += 1;
                if let Some(Value::Hash(h)) = attr(2) {
                    titles.insert(h.0);
                }
            } else if o.class == job_info {
                kinds.insert(attr(0).and_then(int).unwrap_or(0));
            } else if o.class == test {
                tests += 1;
                at_least += usize::from(attr(0).and_then(int) == Some(1));
            }
        }
    }
    assert_eq!((checkpoints, numbers.len(), chapters.len()), (166, 166, 69));
    let streams: Vec<_> = streams.iter().map(|(s, n)| (s.as_str(), *n)).collect();
    assert_eq!(
        streams,
        [
            ("gf2/cuba", 27),
            ("gf2/globals_global", 8),
            ("gf2/miami", 56),
            ("gf2/queens", 75)
        ]
    );
    // "New Objective:" for every objective of the game.
    assert_eq!(objectives, 124);
    assert_eq!(titles.into_iter().collect::<Vec<_>>(), [0x03f4_daba]);
    // 2: the task of a favour, 5: its giver; the kinds of mission tasks (0, 1) are not used.
    assert_eq!(kinds.into_iter().collect::<Vec<_>>(), [2, 5]);
    assert_eq!((tests, at_least), (67, 59));
}
