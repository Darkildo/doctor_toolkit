//! GUIDs of the objects a project adds, derived from their names.
//!
//! A project names the objects it adds (`patches/`); the GUID is the
//! canonical NeoGuid of the tool ([`crate::resource::neoguid`],
//! `docs/formats/simgroup.md` §5) for the text `<scope>/<name>`, where the
//! scope is the mod id and the SimGroup (`my-mod:simgroup:00000004`). The
//! same name gives the same GUID on every build, so references to the object
//! survive; a GUID that is taken steps to the next attempt of
//! [`neoguid::new_guid`].

use crate::resource::neoguid::{self, MachineName};
use crate::resource::simgroup::Guid;

/// The GUID of the new object `name` in `scope`; GUIDs for which `taken` is
/// true are skipped.
pub fn derived(scope: &str, name: &str, taken: impl Fn(&Guid) -> bool) -> Guid {
    neoguid::new_guid(&MachineName::TOOLKIT, &format!("{scope}/{name}"), taken)
}

/// The text form `<machine>_<key1>_<key2>_v3` of a NeoGuid.
pub fn text(guid: &Guid) -> Option<String> {
    neoguid::text(guid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_guids_depend_on_scope_and_name_only() {
        let a = derived("mod:simgroup:00000001", "lamp", |_| false);
        assert_eq!(a, derived("mod:simgroup:00000001", "lamp", |_| false));
        assert_ne!(a, derived("mod:simgroup:00000001", "lamp2", |_| false));
        assert_ne!(a, derived("other:simgroup:00000001", "lamp", |_| false));
        let (machine, _, _) = neoguid::parts(&a).unwrap();
        assert_eq!(machine, MachineName::TOOLKIT);
        assert!(text(&a).unwrap().starts_with("XDOCTORTKX_"));
        let b = derived("mod:simgroup:00000001", "lamp", |g| *g == a);
        assert_ne!(a, b);
        assert_eq!(
            b,
            neoguid::guid_for_name(&MachineName::TOOLKIT, "mod:simgroup:00000001/lamp", 1)
        );
    }
}
