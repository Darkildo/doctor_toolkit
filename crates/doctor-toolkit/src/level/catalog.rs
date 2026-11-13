//! The list of zones a level view can load: streams with SimGroups.

use crate::game::{GameInstance, ResourceIndex};

use super::TYPE_SIMGROUP;

/// A stream with placed objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub stream: usize,
    /// Stream name: lower case, forward slashes (`gf2/miami/local/bonita/high/terrain06`).
    pub name: String,
    pub simgroups: usize,
    pub meshes: usize,
    pub metamodels: usize,
    /// Decompressed size of all resources.
    pub bytes: u64,
}

impl Zone {
    /// The level the zone belongs to: the first two path components
    /// (`gf2/miami`, `mp_mall/mp_mall`).
    pub fn level(&self) -> &str {
        let mut parts = self.name.splitn(3, '/');
        let first = parts.next().unwrap_or_default();
        let second = parts.next();
        match second {
            Some(second) => &self.name[..first.len() + 1 + second.len()],
            None => first,
        }
    }

    /// The directory of the stream (`gf2/miami/local/bonita/high`).
    pub fn dir(&self) -> &str {
        self.name.rsplit_once('/').map_or("", |(d, _)| d)
    }
}

/// Every zone of the game, sorted by name.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub zones: Vec<Zone>,
}

impl Catalog {
    pub fn new(game: &GameInstance, index: &ResourceIndex) -> Self {
        let mut zones: Vec<Zone> = game
            .streams()
            .iter()
            .enumerate()
            .map(|(stream, file)| Zone {
                stream,
                name: file.stream_name(),
                simgroups: 0,
                meshes: 0,
                metamodels: 0,
                bytes: 0,
            })
            .collect();
        for e in &index.entries {
            let zone = &mut zones[e.stream as usize];
            zone.bytes += e.header.data_size as u64;
            match e.header.type_id {
                TYPE_SIMGROUP => zone.simgroups += 1,
                super::TYPE_MESH => zone.meshes += 1,
                super::TYPE_METAMODEL => zone.metamodels += 1,
                _ => {}
            }
        }
        zones.retain(|z| z.simgroups > 0);
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        Self { zones }
    }

    /// Zones whose name starts with `prefix` (a directory or a stream).
    pub fn under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a Zone> + 'a {
        self.zones.iter().filter(move |z| {
            z.name == prefix
                || (z.name.starts_with(prefix)
                    && z.name.as_bytes().get(prefix.len()) == Some(&b'/'))
        })
    }

    /// The levels (first two path components) in order.
    pub fn levels(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.zones.iter().map(Zone::level).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_directories() {
        let zone = Zone {
            stream: 0,
            name: "gf2/miami/local/bonita/high/terrain06".into(),
            simgroups: 1,
            meshes: 0,
            metamodels: 0,
            bytes: 0,
        };
        assert_eq!(zone.level(), "gf2/miami");
        assert_eq!(zone.dir(), "gf2/miami/local/bonita/high");
        let top = Zone {
            name: "gf2/frontend".into(),
            ..zone.clone()
        };
        assert_eq!(top.level(), "gf2/frontend");
        let catalog = Catalog {
            zones: vec![top, zone],
        };
        assert_eq!(catalog.under("gf2/miami/local").count(), 1);
        assert_eq!(catalog.under("gf2/miami/loc").count(), 0);
        assert_eq!(catalog.under("gf2/frontend").count(), 1);
        assert_eq!(catalog.levels(), ["gf2/frontend", "gf2/miami"]);
    }
}
