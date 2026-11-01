use std::collections::HashMap;

use tile_core::{
    Effect, LayoutKind, LayoutSettings, ManageOptions, MonitorInfo, State, WindowId, WindowInfo,
};
use tile_platform::desktops::DesktopId;

/// One tiling [`State`] (workspaces + layouts) per native virtual desktop.
///
/// Windows on other desktops are cloaked by the shell, not closed, so they stay managed by
/// their desktop's state and keep their place in its layouts.
pub struct Desktops {
    states: HashMap<DesktopId, State>,
    current: DesktopId,
    owner: HashMap<WindowId, DesktopId>,
    settings: LayoutSettings,
    layout: LayoutKind,
    monitors: Vec<MonitorInfo>,
}

/// Effects from a background desktop must never steal focus.
fn background(mut effects: Vec<Effect>) -> Vec<Effect> {
    effects.retain(|e| !matches!(e, Effect::Focus(_)));
    effects
}

impl Desktops {
    pub fn new(current: DesktopId, settings: LayoutSettings, layout: LayoutKind) -> Self {
        let mut states = HashMap::new();
        states.insert(current, State::new(settings.clone(), layout));
        Self {
            states,
            current,
            owner: HashMap::new(),
            settings,
            layout,
            monitors: Vec::new(),
        }
    }

    pub const fn current_id(&self) -> DesktopId {
        self.current
    }

    fn state_mut(&mut self, desktop: DesktopId) -> &mut State {
        let (settings, layout, monitors) = (&self.settings, self.layout, &self.monitors);
        self.states.entry(desktop).or_insert_with(|| {
            let mut state = State::new(settings.clone(), layout);
            state.set_monitors(monitors.clone());
            state
        })
    }

    pub fn current(&mut self) -> &mut State {
        self.state_mut(self.current)
    }

    pub fn current_ref(&self) -> &State {
        // The current desktop's state is created on construction and on every switch.
        &self.states[&self.current]
    }

    pub fn owner(&self, id: WindowId) -> Option<DesktopId> {
        self.owner.get(&id).copied()
    }

    pub fn is_managed(&self, id: WindowId) -> bool {
        self.owner.contains_key(&id)
    }

    pub fn managed(&self) -> Vec<WindowId> {
        self.owner.keys().copied().collect()
    }

    /// Windows hidden by any desktop's workspaces.
    pub fn hidden(&self) -> Vec<WindowId> {
        self.states.values().flat_map(State::hidden).collect()
    }

    fn owner_ref(&self, id: WindowId) -> Option<&State> {
        self.states.get(&self.owner(id)?)
    }

    pub fn is_hidden(&self, id: WindowId) -> bool {
        self.owner_ref(id).is_some_and(|s| s.is_hidden(id))
    }

    pub fn is_floating(&self, id: WindowId) -> bool {
        self.owner_ref(id).is_some_and(|s| s.is_floating(id))
    }

    /// Runs `f` against the state that manages `id`.
    pub fn with_owner(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut State) -> Vec<Effect>,
    ) -> Vec<Effect> {
        let Some(desktop) = self.owner(id) else {
            return Vec::new();
        };
        let effects = f(self.state_mut(desktop));
        if desktop == self.current {
            effects
        } else {
            background(effects)
        }
    }

    pub fn open(
        &mut self,
        desktop: DesktopId,
        id: WindowId,
        info: WindowInfo,
        opts: ManageOptions,
    ) -> Vec<Effect> {
        self.owner.insert(id, desktop);
        let effects = self.state_mut(desktop).window_opened(id, info, opts);
        if desktop == self.current {
            effects
        } else {
            background(effects)
        }
    }

    pub fn close(&mut self, id: WindowId) -> Vec<Effect> {
        let effects = self.with_owner(id, |s| s.window_closed(id));
        self.owner.remove(&id);
        effects
    }

    pub fn switch_to(&mut self, desktop: DesktopId) -> Vec<Effect> {
        self.current = desktop;
        self.current().sync()
    }

    fn for_all(&mut self, mut f: impl FnMut(&mut State) -> Vec<Effect>) -> Vec<Effect> {
        let current = self.current;
        self.states
            .iter_mut()
            .flat_map(|(desktop, state)| {
                let effects = f(state);
                if *desktop == current {
                    effects
                } else {
                    background(effects)
                }
            })
            .collect()
    }

    pub fn set_monitors(&mut self, monitors: Vec<MonitorInfo>) -> Vec<Effect> {
        self.monitors.clone_from(&monitors);
        self.for_all(|s| s.set_monitors(monitors.clone()))
    }

    pub fn set_settings(&mut self, settings: LayoutSettings, layout: LayoutKind) -> Vec<Effect> {
        self.settings = settings.clone();
        self.layout = layout;
        self.for_all(|s| s.set_settings(settings.clone(), layout))
    }
}

#[cfg(test)]
mod tests {
    use tile_core::{MonitorId, geometry::Rect};

    use super::*;

    const ONE: DesktopId = 1;
    const TWO: DesktopId = 2;
    const W: WindowId = WindowId(1);
    const X: WindowId = WindowId(2);

    fn desktops() -> Desktops {
        let mut d = Desktops::new(ONE, LayoutSettings::default(), LayoutKind::Dwindle);
        d.set_monitors(vec![MonitorInfo {
            id: MonitorId(1),
            rect: Rect::new(0, 0, 1000, 1000),
            work_area: Rect::new(0, 0, 1000, 960),
        }]);
        d
    }

    fn open(d: &mut Desktops, desktop: DesktopId, id: WindowId) -> Vec<Effect> {
        d.open(
            desktop,
            id,
            WindowInfo::default(),
            ManageOptions {
                focus: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn a_window_belongs_to_the_desktop_it_opened_on() {
        let mut d = desktops();
        open(&mut d, ONE, W);
        open(&mut d, TWO, X);
        assert_eq!(d.owner(W), Some(ONE));
        assert_eq!(d.owner(X), Some(TWO));
        assert!(d.is_managed(W) && d.is_managed(X));
        assert_eq!(d.managed().len(), 2);
        assert_eq!(d.current_id(), ONE);
    }

    #[test]
    fn the_current_desktop_may_focus_but_others_may_not() {
        let mut d = desktops();
        let here = open(&mut d, ONE, W);
        assert!(here.contains(&Effect::Focus(W)));
        let elsewhere = open(&mut d, TWO, X);
        assert!(!elsewhere.iter().any(|e| matches!(e, Effect::Focus(_))));
        assert!(
            d.with_owner(X, |s| s.window_focused(X))
                .iter()
                .all(|e| !matches!(e, Effect::Focus(_)))
        );
    }

    #[test]
    fn unknown_windows_have_no_owner_and_no_effects() {
        let mut d = desktops();
        assert_eq!(d.owner(W), None);
        assert!(d.with_owner(W, |s| s.window_closed(W)).is_empty());
        assert!(d.close(W).is_empty());
        assert!(!d.is_hidden(W));
        assert!(!d.is_floating(W));
    }

    #[test]
    fn closing_forgets_the_window() {
        let mut d = desktops();
        open(&mut d, ONE, W);
        d.close(W);
        assert!(!d.is_managed(W));
        assert!(!d.current_ref().is_managed(W));
    }

    #[test]
    fn switching_resyncs_the_new_desktop() {
        let mut d = desktops();
        open(&mut d, TWO, X);
        let effects = d.switch_to(TWO);
        assert_eq!(d.current_id(), TWO);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Place { id, .. } if *id == X))
        );
        assert!(d.current_ref().is_managed(X));
    }

    #[test]
    fn a_new_desktop_inherits_the_monitors() {
        let mut d = desktops();
        d.switch_to(TWO);
        assert_eq!(d.current_ref().monitor_count(), 1);
    }

    #[test]
    fn hidden_windows_are_collected_from_every_desktop() {
        let mut d = desktops();
        open(&mut d, ONE, W);
        d.current().apply(&tile_core::Command::Workspace(2));
        assert_eq!(d.hidden(), vec![W]);
        assert!(d.is_hidden(W));
    }

    #[test]
    fn floating_is_looked_up_on_the_owner() {
        let mut d = desktops();
        d.open(
            TWO,
            W,
            WindowInfo::default(),
            ManageOptions {
                floating: true,
                rect: Rect::new(10, 10, 300, 200),
                ..Default::default()
            },
        );
        assert!(d.is_floating(W));
    }

    #[test]
    fn monitor_and_settings_changes_reach_every_desktop() {
        let mut d = desktops();
        open(&mut d, ONE, W);
        open(&mut d, TWO, X);
        let effects = d.set_monitors(vec![MonitorInfo {
            id: MonitorId(1),
            rect: Rect::new(0, 0, 2000, 1000),
            work_area: Rect::new(0, 0, 2000, 960),
        }]);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Place { id, .. } if *id == W))
        );
        let settings = LayoutSettings {
            gaps_out: 30,
            ..LayoutSettings::default()
        };
        let effects = d.set_settings(settings, LayoutKind::Even);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Place { id, .. } if *id == W))
        );
        assert!(!effects.iter().any(|e| matches!(e, Effect::Focus(_))));
    }
}
