//! Shared space-selector state.
//!
//! The sidebar draws the selector and the app owns settings, where the spaces
//! actually live — and the two hold no handle to each other. Both can see
//! `okena-workspace`, so the list lives here as an [`Entity`] behind a global,
//! the same shape as the active-harness-view state: the app publishes it
//! whenever settings change, and the sidebar `cx.observe`s it and repaints.
//!
//! Only what the selector draws. A space's roots, filters and connection are
//! settings, read where they are used.

use gpui::*;
use okena_core::spaces::SpaceData;

/// The spaces and which one is showing.
#[derive(Default)]
pub struct SpacesState {
    spaces: Vec<SpaceData>,
    active: String,
}

impl SpacesState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spaces(&self) -> &[SpaceData] {
        &self.spaces
    }

    pub fn active(&self) -> &str {
        &self.active
    }

    /// Where the active space sits, counting from 0. `None` before the app has
    /// published anything.
    pub fn active_index(&self) -> Option<usize> {
        self.spaces.iter().position(|s| s.id == self.active)
    }

    /// Replace the list, notifying observers only on a real change so every
    /// unrelated settings edit does not repaint the sidebar.
    pub fn set(&mut self, spaces: Vec<SpaceData>, active: String, cx: &mut Context<Self>) {
        if self.spaces == spaces && self.active == active {
            return;
        }
        self.spaces = spaces;
        self.active = active;
        cx.notify();
    }
}

/// Global handle to the shared [`SpacesState`].
pub struct GlobalSpacesState(pub Entity<SpacesState>);

impl Global for GlobalSpacesState {}

/// Handle to the shared state, when it has been registered.
pub fn spaces_state_entity(cx: &App) -> Option<Entity<SpacesState>> {
    cx.try_global::<GlobalSpacesState>().map(|g| g.0.clone())
}

/// The spaces to draw, in selector order. Empty in a headless context where
/// nothing has published them.
pub fn spaces(cx: &App) -> Vec<SpaceData> {
    cx.try_global::<GlobalSpacesState>()
        .map(|g| g.0.read(cx).spaces().to_vec())
        .unwrap_or_default()
}

/// Which space is showing.
pub fn active_space(cx: &App) -> String {
    cx.try_global::<GlobalSpacesState>()
        .map(|g| g.0.read(cx).active().to_string())
        .unwrap_or_else(okena_core::spaces::default_space_id)
}

/// Publish the space list. A no-op when the global has not been registered.
pub fn publish_spaces(spaces: Vec<SpaceData>, active: String, cx: &mut App) {
    let Some(entity) = spaces_state_entity(cx) else {
        return;
    };
    entity.update(cx, |state, cx| state.set(spaces, active, cx));
}

#[cfg(test)]
mod tests {
    // Deliberately not `use super::*`: that would re-export the `gpui::*` glob
    // and shadow std's `#[test]` with gpui's own attribute macro.
    use super::SpacesState;
    use okena_core::spaces::SpaceData;

    fn state(ids: &[&str], active: &str) -> SpacesState {
        SpacesState {
            spaces: ids.iter().map(|id| SpaceData::new(*id, *id)).collect(),
            active: active.to_string(),
        }
    }

    #[test]
    fn the_active_index_is_where_the_highlight_goes() {
        assert_eq!(state(&["default", "a", "b"], "b").active_index(), Some(2));
    }

    #[test]
    fn an_unknown_active_space_highlights_nothing() {
        // Rather than highlighting the wrong dot while a switch is in flight.
        assert!(state(&["default"], "gone").active_index().is_none());
        assert!(SpacesState::new().active_index().is_none());
    }
}
