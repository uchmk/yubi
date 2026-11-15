//! The action resolver: the one place the engine asks "what does this gesture do right now".
//!
//! The feature tables in `Config` stay the single authority. A per-app profile may override single
//! gestures; the match is the exe basename, case-insensitive and exact (no globs).

use super::types::GestureId;
use crate::config::{Action, Config, EdgeTarget, Profile};

/// The exe basename of `exe` as profiles compare it: trimmed, and without any directory part.
pub fn basename(exe: &str) -> &str {
    let t = exe.trim();
    t.rsplit(['\\', '/']).next().unwrap_or(t)
}

/// True if the profile is the one for `exe`.
pub fn profile_matches(p: &Profile, exe: &str) -> bool {
    let name = basename(&p.exe_name);
    !name.is_empty() && name.eq_ignore_ascii_case(basename(exe))
}

/// The first profile that names `exe`. (Duplicates are rejected on import and dropped on load, so
/// "first" only matters for hand-built configs.)
pub fn matching_profile<'a>(profiles: &'a [Profile], exe: Option<&str>) -> Option<&'a Profile> {
    let exe = exe?;
    profiles.iter().find(|p| profile_matches(p, exe))
}

/// True if a profile for `exe` turns yubi off for that app.
pub fn app_blocked(profiles: &[Profile], exe: Option<&str>) -> bool {
    matching_profile(profiles, exe).is_some_and(|p| !p.enabled)
}

/// What `id` does while `exe` is in the foreground: the profile's override if it has one (an
/// explicit `None` disables the gesture for that app), otherwise the global binding.
pub fn resolve(cfg: &Config, exe: Option<&str>, id: &GestureId) -> Action {
    if let Some(p) = matching_profile(&cfg.profiles, exe)
        && p.enabled
        && let Some(a) = p.overrides.get(id.as_str())
    {
        return a.clone();
    }
    cfg.binding(id).unwrap_or_default()
}

/// The target of an edge slider (`key` is `edge.left`, `edge.right` or `edge.top`) for `exe`.
pub fn resolve_edge(cfg: &Config, exe: Option<&str>, key: &str) -> EdgeTarget {
    if let Some(p) = matching_profile(&cfg.profiles, exe)
        && p.enabled
        && let Some(t) = p.edge_overrides.get(key)
    {
        return t.clone();
    }
    match key {
        "edge.left" => cfg.edge.left.clone(),
        "edge.right" => cfg.edge.right.clone(),
        "edge.top" => cfg.edge.top.clone(),
        _ => EdgeTarget::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(exe: &str, enabled: bool) -> Profile {
        Profile {
            exe_name: exe.to_owned(),
            enabled,
            ..Profile::default()
        }
    }

    fn cfg_with(p: Profile) -> Config {
        let mut cfg = Config::default();
        cfg.four.swipe_left = Action::Find;
        cfg.edge.left = EdgeTarget::Volume;
        cfg.profiles.push(p);
        cfg
    }

    #[test]
    fn global_binding_without_profile() {
        let cfg = Config::default();
        let id = GestureId::swipe(4, super::super::types::SwipeDir::Left);
        assert_eq!(resolve(&cfg, None, &id), Action::DesktopNext);
        assert_eq!(resolve(&cfg, Some("code.exe"), &id), Action::DesktopNext);
        // An id without a field resolves to none.
        assert_eq!(resolve(&cfg, None, &GestureId::tap(3)), Action::None);
    }

    #[test]
    fn override_wins_for_the_exact_basename_case_insensitively() {
        let mut p = profile("Code.EXE", true);
        p.overrides
            .insert("swipe.4.left".into(), Action::Keys("Ctrl+PageUp".into()));
        let cfg = cfg_with(p);
        let id = GestureId::parse("swipe.4.left").unwrap();
        let own = Action::Keys("Ctrl+PageUp".into());
        assert_eq!(resolve(&cfg, Some("code.exe"), &id), own);
        assert_eq!(resolve(&cfg, Some("CODE.exe"), &id), own);
        assert_eq!(resolve(&cfg, Some(r"C:\Apps\code.exe"), &id), own);
        // Exact: no prefix, suffix or glob matching.
        assert_eq!(resolve(&cfg, Some("code"), &id), Action::Find);
        assert_eq!(resolve(&cfg, Some("code.exe.bak"), &id), Action::Find);
        assert_eq!(resolve(&cfg, Some("xcode.exe"), &id), Action::Find);
        assert_eq!(resolve(&cfg, None, &id), Action::Find);
        // Other gestures of the same app are untouched.
        let other = GestureId::parse("swipe.4.right").unwrap();
        assert_eq!(resolve(&cfg, Some("code.exe"), &other), Action::DesktopPrev);
    }

    #[test]
    fn explicit_none_disables_the_gesture_for_that_app() {
        let mut p = profile("code.exe", true);
        p.overrides.insert("swipe.4.left".into(), Action::None);
        let cfg = cfg_with(p);
        let id = GestureId::parse("swipe.4.left").unwrap();
        assert_eq!(resolve(&cfg, Some("code.exe"), &id), Action::None);
        assert_eq!(resolve(&cfg, Some("other.exe"), &id), Action::Find);
    }

    #[test]
    fn disabled_profiles_block_and_do_not_override() {
        let mut p = profile("game.exe", false);
        p.overrides.insert("swipe.4.left".into(), Action::TaskView);
        let cfg = cfg_with(p);
        assert!(app_blocked(&cfg.profiles, Some("GAME.exe")));
        assert!(!app_blocked(&cfg.profiles, Some("other.exe")));
        assert!(!app_blocked(&cfg.profiles, None));
        let id = GestureId::parse("swipe.4.left").unwrap();
        assert_eq!(resolve(&cfg, Some("game.exe"), &id), Action::Find);
    }

    #[test]
    fn first_profile_wins_and_empty_names_never_match() {
        let list = [
            profile("a.exe", true),
            profile("A.EXE", false),
            profile("", true),
        ];
        assert!(matching_profile(&list, Some("a.exe")).unwrap().enabled);
        assert!(!app_blocked(&list, Some("a.exe")));
        assert!(matching_profile(&list, Some("")).is_none());
        assert!(matching_profile(&list, Some("  ")).is_none());
    }

    #[test]
    fn edge_overrides_resolve_per_edge() {
        let mut p = profile("code.exe", true);
        p.edge_overrides
            .insert("edge.left".into(), EdgeTarget::Scroll);
        p.edge_overrides.insert("edge.top".into(), EdgeTarget::None);
        let mut cfg = cfg_with(p);
        cfg.edge.top = EdgeTarget::Zoom;
        assert_eq!(
            resolve_edge(&cfg, Some("code.exe"), "edge.left"),
            EdgeTarget::Scroll
        );
        assert_eq!(
            resolve_edge(&cfg, Some("code.exe"), "edge.top"),
            EdgeTarget::None
        );
        assert_eq!(
            resolve_edge(&cfg, Some("code.exe"), "edge.right"),
            EdgeTarget::None
        );
        assert_eq!(
            resolve_edge(&cfg, Some("x.exe"), "edge.left"),
            EdgeTarget::Volume
        );
        assert_eq!(
            resolve_edge(&cfg, Some("x.exe"), "edge.top"),
            EdgeTarget::Zoom
        );
        assert_eq!(resolve_edge(&cfg, None, "edge.bogus"), EdgeTarget::None);
    }
}
