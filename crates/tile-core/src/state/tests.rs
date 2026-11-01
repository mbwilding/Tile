use super::*;
use crate::{
    Command, MonitorId, WindowId,
    geometry::{Direction, Rect},
    layout::{LayoutKind, LayoutSettings},
};

const W: WindowId = WindowId(1);
const X: WindowId = WindowId(2);
const Y: WindowId = WindowId(3);

fn monitor(id: isize, x: i32) -> MonitorInfo {
    MonitorInfo {
        id: MonitorId(id),
        rect: Rect::new(x, 0, 1000, 1000),
        work_area: Rect::new(x, 0, 1000, 960),
    }
}

fn state(monitors: usize) -> State {
    state_with(monitors, true)
}

fn state_with(monitors: usize, independent: bool) -> State {
    let settings = LayoutSettings {
        gaps_in: 0,
        gaps_out: 0,
        independent_workspaces: independent,
        ..LayoutSettings::default()
    };
    let mut s = State::new(settings, LayoutKind::Dwindle);
    let infos = (0..monitors)
        .map(|i| monitor(i as isize + 1, i as i32 * 1000))
        .collect();
    s.set_monitors(infos);
    s
}

fn open(s: &mut State, id: WindowId) -> Vec<Effect> {
    s.window_opened(
        id,
        WindowInfo::default(),
        ManageOptions {
            focus: true,
            ..Default::default()
        },
    )
}

fn placed(effects: &[Effect], id: WindowId) -> Option<Rect> {
    effects.iter().rev().find_map(|e| match e {
        Effect::Place { id: i, rect } if *i == id => Some(*rect),
        _ => None,
    })
}

#[test]
fn monitors_have_workspaces_of_their_own() {
    let s = state(2);
    let ws: Vec<_> = s.snapshot_monitors().iter().map(|m| m.workspace).collect();
    assert_eq!(ws, vec![1, 1]);
    assert_eq!(s.snapshot_workspaces().len(), 2);
}

#[test]
fn switching_workspace_stays_on_the_monitor() {
    let mut s = state(2);
    open(&mut s, W);
    let effects = s.apply(&Command::Workspace(2));
    assert!(effects.contains(&Effect::Hide(W)));
    let monitors = s.snapshot_monitors();
    assert!(monitors[0].focused);
    assert_eq!((monitors[0].workspace, monitors[1].workspace), (2, 1));
}

#[test]
fn an_empty_workspace_keeps_focus_on_its_monitor() {
    let mut s = state(2);
    open(&mut s, W);
    s.apply(&Command::MoveFocus(Direction::Right));
    open(&mut s, X);
    s.apply(&Command::MoveFocus(Direction::Left));
    let effects = s.apply(&Command::Workspace(2));
    assert!(effects.contains(&Effect::Defocus));
    assert!(s.snapshot_monitors()[0].focused);
    assert_eq!(s.focused(), None);
}

#[test]
fn synced_monitors_switch_together() {
    let mut s = state_with(2, false);
    s.apply(&Command::Workspace(3));
    let ws: Vec<_> = s.snapshot_monitors().iter().map(|m| m.workspace).collect();
    assert_eq!(ws, vec![3, 3]);
}

#[test]
fn opening_tiles_and_focuses() {
    let mut s = state(1);
    open(&mut s, W);
    let effects = open(&mut s, X);
    assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 500, 960)));
    assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
    assert_eq!(s.focused(), Some(X));
}

#[test]
fn workspace_switch_hides_and_shows() {
    let mut s = state(1);
    open(&mut s, W);
    let effects = s.apply(&Command::Workspace(2));
    assert!(effects.contains(&Effect::Hide(W)));
    assert_eq!(s.focused(), None);
    let effects = s.apply(&Command::Workspace(1));
    assert!(effects.contains(&Effect::Show(W)));
    assert!(effects.contains(&Effect::Focus(W)));
}

#[test]
fn move_to_workspace_follows_the_window() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.apply(&Command::MoveToWorkspace(3));
    assert!(effects.contains(&Effect::Focus(X)));
    assert!(effects.contains(&Effect::Hide(W)));
    assert_eq!(s.focused(), Some(X));
    assert_eq!(s.focused_workspace(), Some(3));
}

#[test]
fn move_to_workspace_silent_keeps_view() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.apply(&Command::MoveToWorkspaceSilent(3));
    assert!(effects.contains(&Effect::Hide(X)));
    assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
    assert_eq!(s.focused(), Some(W));
    assert_eq!(s.focused_workspace(), Some(1));
}

#[test]
fn focus_crosses_monitors() {
    let mut s = state(2);
    open(&mut s, W);
    let effects = s.apply(&Command::MoveFocus(Direction::Right));
    assert!(s.snapshot_monitors()[1].focused);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Focus(_))));
    s.apply(&Command::MoveFocus(Direction::Left));
    assert_eq!(s.focused(), Some(W));
}

#[test]
fn focus_enters_the_nearest_window_on_the_next_monitor() {
    let mut s = state(2);
    open(&mut s, W);
    s.apply(&Command::MoveFocus(Direction::Right));
    open(&mut s, X);
    open(&mut s, Y);
    s.apply(&Command::MoveFocus(Direction::Left));
    s.apply(&Command::MoveFocus(Direction::Left));
    assert_eq!(s.focused(), Some(W));
    let effects = s.apply(&Command::MoveFocus(Direction::Right));
    assert!(effects.contains(&Effect::Focus(X)));
    assert_eq!(s.focused(), Some(X));
    s.apply(&Command::MoveFocus(Direction::Right));
    s.apply(&Command::MoveFocus(Direction::Left));
    assert_eq!(s.focused(), Some(X));
}

#[test]
fn portrait_monitors_start_with_even() {
    let mut s = State::new(LayoutSettings::default(), LayoutKind::Dwindle);
    s.set_monitors(vec![
        monitor(1, 0),
        MonitorInfo {
            id: MonitorId(2),
            rect: Rect::new(1000, 0, 1080, 1920),
            work_area: Rect::new(1000, 0, 1080, 1880),
        },
    ]);
    let layouts: Vec<_> = s.snapshot_workspaces().iter().map(|w| w.layout).collect();
    assert_eq!(layouts, vec![LayoutKind::Dwindle, LayoutKind::Even]);
    s.set_monitors(vec![monitor(1, 0), monitor(2, 1000)]);
    assert_eq!(s.snapshot_workspaces()[1].layout, LayoutKind::Dwindle);
}

#[test]
fn move_window_to_other_monitor_at_edge() {
    let mut s = state(2);
    open(&mut s, W);
    let effects = s.apply(&Command::MoveWindow(Direction::Right));
    assert_eq!(placed(&effects, W), Some(Rect::new(1000, 0, 1000, 960)));
    assert!(s.snapshot_monitors()[1].focused);
}

#[test]
fn closing_focuses_previous() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    open(&mut s, Y);
    s.window_focused(X);
    let effects = s.window_closed(X);
    assert!(effects.contains(&Effect::Focus(Y)));
}

#[test]
fn cycle_wraps_in_layout_order() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    open(&mut s, Y);
    assert!(s.apply(&Command::CycleNext).contains(&Effect::Focus(W)));
    assert!(s.apply(&Command::CyclePrev).contains(&Effect::Focus(Y)));
}

#[test]
fn floating_and_fullscreen() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.apply(&Command::ToggleFloating);
    assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
    assert!(s.is_floating(X));
    s.window_focused(W);
    let effects = s.apply(&Command::Fullscreen);
    assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 1000)));
}

#[test]
fn fullscreen_owns_the_workspace() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    s.apply(&Command::Fullscreen);
    // Focus can't wander to windows hidden behind the fullscreen one.
    assert!(s.apply(&Command::MoveFocus(Direction::Left)).is_empty());
    assert!(s.apply(&Command::CycleNext).is_empty());
    assert!(s.apply(&Command::MoveWindow(Direction::Left)).is_empty());
    assert_eq!(s.focused(), Some(X));
    // A new window taking focus ends fullscreen, so it is actually visible.
    let effects = open(&mut s, Y);
    assert_ne!(placed(&effects, X), Some(Rect::new(0, 0, 1000, 1000)));
}

#[test]
fn minimize_and_restore() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.window_minimized(X);
    assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
    let effects = s.window_restored(X);
    assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
}

#[test]
fn scrolling_hides_offscreen_columns() {
    let mut s = state(1);
    s.apply(&Command::Layout(LayoutKind::Scrolling));
    open(&mut s, W);
    open(&mut s, X);
    let effects = open(&mut s, Y);
    assert!(effects.contains(&Effect::Hide(W)));
    let effects = s.apply(&Command::MoveFocus(Direction::Left));
    let effects = [effects, s.apply(&Command::MoveFocus(Direction::Left))].concat();
    assert!(effects.contains(&Effect::Show(W)));
    assert_eq!(s.focused(), Some(W));
}

#[test]
fn monitor_removal_keeps_windows() {
    let mut s = state(2);
    s.apply(&Command::FocusMonitor(Direction::Right));
    open(&mut s, W);
    let effects = s.set_monitors(vec![monitor(1, 0)]);
    assert_eq!(s.monitor_count(), 1);
    assert_eq!(placed(&effects, W), Some(Rect::new(0, 0, 1000, 960)));
}

#[test]
fn dragging_an_edge_over_a_neighbour_resizes_instead_of_swapping() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    // W's right edge dragged well into X, the pointer ending over X.
    let effects = s.window_moved(W, Rect::new(0, 0, 700, 960), (690, 400));
    assert_eq!(placed(&effects, W).map(|r| (r.x, r.w)), Some((0, 700)));
    assert_eq!(placed(&effects, X).map(|r| r.x), Some(700));
}

#[test]
fn dragging_an_edge_moves_the_others_as_it_goes() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    assert!(s.window_resizing(W, Rect::new(0, 0, 500, 960)).is_empty());
    let effects = s.window_resizing(W, Rect::new(0, 0, 600, 960));
    assert_eq!(placed(&effects, X).map(|r| r.x), Some(600));
    let settled = s.window_resized(W, Rect::new(0, 0, 600, 960));
    assert_eq!(placed(&settled, W).map(|r| r.w), Some(600));
}

#[test]
fn drag_onto_other_tile_swaps() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.window_moved(W, Rect::new(600, 10, 500, 960), (700, 100));
    assert_eq!(placed(&effects, W), Some(Rect::new(500, 0, 500, 960)));
}

fn float(s: &mut State, id: WindowId) {
    s.window_opened(
        id,
        WindowInfo::default(),
        ManageOptions {
            floating: true,
            rect: Rect::new(100, 100, 400, 300),
            focus: true,
            ..Default::default()
        },
    );
}

#[test]
fn opening_a_managed_window_again_does_nothing() {
    let mut s = state(1);
    open(&mut s, W);
    assert!(open(&mut s, W).is_empty());
    assert_eq!(s.managed().count(), 1);
}

#[test]
fn nothing_happens_without_monitors() {
    let mut s = State::new(LayoutSettings::default(), LayoutKind::Dwindle);
    assert!(open(&mut s, W).is_empty());
    assert!(s.apply(&Command::Workspace(2)).is_empty());
    assert!(s.focused_monitor().is_none());
    assert!(s.focused_workspace().is_none());
    assert!(s.active_layout().is_none());
}

#[test]
fn exec_and_kill_become_effects() {
    let mut s = state(1);
    assert_eq!(
        s.apply(&Command::Exec("calc".into())),
        vec![Effect::Exec("calc".into())]
    );
    assert!(s.apply(&Command::KillActive).is_empty());
    open(&mut s, W);
    assert_eq!(s.apply(&Command::KillActive), vec![Effect::Close(W)]);
}

#[test]
fn commands_for_the_daemon_have_no_effects() {
    let mut s = state(1);
    open(&mut s, W);
    for command in [
        Command::FocusOrExec("x".into()),
        Command::Pin,
        Command::Lock,
        Command::Launcher,
        Command::Reload,
        Command::TogglePause,
        Command::ToggleTile,
        Command::ToggleRemap,
        Command::ToggleShell,
        Command::ToggleLog,
        Command::Exit,
    ] {
        assert!(s.apply(&command).is_empty(), "{command:?}");
    }
}

#[test]
fn on_focused_monitor_follows_focus() {
    let mut s = state(2);
    open(&mut s, W);
    s.apply(&Command::FocusMonitor(Direction::Right));
    open(&mut s, X);
    assert!(s.on_focused_monitor(X));
    assert!(!s.on_focused_monitor(W));
    assert!(!s.on_focused_monitor(Y));
}

#[test]
fn a_window_opened_for_another_workspace_is_hidden() {
    let mut s = state(1);
    let effects = s.window_opened(
        W,
        WindowInfo::default(),
        ManageOptions {
            workspace: Some(4),
            focus: true,
            ..Default::default()
        },
    );
    assert!(!effects.iter().any(|e| matches!(e, Effect::Focus(_))));
    assert!(s.is_hidden(W));
    assert_eq!(s.focused_workspace(), Some(1));
}

#[test]
fn a_window_opened_for_another_monitor_goes_there() {
    let mut s = state(2);
    let effects = s.window_opened(
        W,
        WindowInfo::default(),
        ManageOptions {
            monitor: Some(1),
            focus: true,
            ..Default::default()
        },
    );
    assert_eq!(placed(&effects, W), Some(Rect::new(1000, 0, 1000, 960)));
    assert!(s.snapshot_monitors()[1].focused);
}

#[test]
fn focus_from_the_os_on_a_hidden_workspace_switches_to_it() {
    let mut s = state(1);
    open(&mut s, W);
    s.apply(&Command::Workspace(2));
    assert!(s.is_hidden(W));
    let effects = s.window_focused(W);
    assert!(effects.contains(&Effect::Focus(W)));
    assert_eq!(s.focused_workspace(), Some(1));
}

#[test]
fn focus_on_an_unmanaged_window_clears_focus() {
    let mut s = state(1);
    open(&mut s, W);
    assert!(s.window_focused(Y).is_empty());
    assert_eq!(s.focused(), None);
}

#[test]
fn focus_on_the_focused_window_is_a_no_op() {
    let mut s = state(1);
    open(&mut s, W);
    assert!(s.window_focused(W).is_empty());
}

#[test]
fn focus_from_the_os_moves_the_focused_monitor() {
    let mut s = state(2);
    open(&mut s, W);
    s.apply(&Command::FocusMonitor(Direction::Right));
    open(&mut s, X);
    s.window_focused(W);
    assert!(s.snapshot_monitors()[0].focused);
}

#[test]
fn closing_the_last_window_defocuses() {
    let mut s = state(1);
    open(&mut s, W);
    let effects = s.window_closed(W);
    assert!(effects.contains(&Effect::Defocus));
    assert_eq!(s.focused(), None);
    assert!(!s.is_managed(W));
}

#[test]
fn closing_an_unfocused_window_keeps_focus() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    s.window_closed(W);
    assert_eq!(s.focused(), Some(X));
    assert!(s.window_closed(Y).is_empty());
}

#[test]
fn closing_a_window_on_a_hidden_workspace_keeps_focus() {
    let mut s = state(1);
    open(&mut s, W);
    s.apply(&Command::Workspace(2));
    open(&mut s, X);
    s.apply(&Command::MoveToWorkspaceSilent(1));
    s.window_closed(X);
    assert!(!s.is_managed(X));
    assert!(s.is_managed(W));
}

#[test]
fn minimising_the_last_window_defocuses() {
    let mut s = state(1);
    open(&mut s, W);
    let effects = s.window_minimized(W);
    assert!(effects.contains(&Effect::Defocus));
    assert!(s.window_minimized(Y).is_empty());
}

#[test]
fn restoring_a_window_that_is_not_minimised_does_nothing() {
    let mut s = state(1);
    open(&mut s, W);
    assert!(s.window_restored(W).is_empty());
    assert!(s.window_restored(Y).is_empty());
}

#[test]
fn dragging_to_another_monitor_moves_the_window() {
    let mut s = state(2);
    open(&mut s, W);
    let effects = s.window_moved(W, Rect::new(1200, 100, 400, 300), (1300, 200));
    assert_eq!(placed(&effects, W), Some(Rect::new(1000, 0, 1000, 960)));
    assert!(s.snapshot_monitors()[1].focused);
}

#[test]
fn dragging_a_floating_window_keeps_its_place() {
    let mut s = state(1);
    float(&mut s, W);
    let effects = s.window_moved(W, Rect::new(200, 220, 400, 300), (300, 300));
    assert_eq!(placed(&effects, W), Some(Rect::new(200, 220, 400, 300)));
    assert!(s.window_moved(Y, Rect::default(), (0, 0)).is_empty());
}

#[test]
fn floating_windows_move_and_resize_in_steps() {
    let mut s = state(1);
    float(&mut s, W);
    let start = placed(&s.sync(), W).unwrap();
    let right = s.apply(&Command::MoveWindow(Direction::Right));
    assert_eq!(placed(&right, W).unwrap().x, start.x + 50);
    let down = s.apply(&Command::MoveWindow(Direction::Down));
    assert!(placed(&down, W).unwrap().y > start.y);
    s.apply(&Command::MoveWindow(Direction::Left));
    s.apply(&Command::MoveWindow(Direction::Up));
    let grown = s.apply(&Command::ResizeActive(40, 20));
    assert_eq!(placed(&grown, W).map(|r| (r.w, r.h)), Some((440, 320)));
    let floor = s.apply(&Command::ResizeActive(-1000, -1000));
    assert_eq!(placed(&floor, W).map(|r| (r.w, r.h)), Some((100, 100)));
}

#[test]
fn resizing_a_tiled_window_changes_its_split() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.apply(&Command::ResizeActive(100, 0));
    let (w, x) = (placed(&effects, W).unwrap(), placed(&effects, X).unwrap());
    assert_ne!(w.w, 500);
    assert_eq!(w.w + x.w, 1000);
    assert!(!s.apply(&Command::ResizeActive(1, 1)).is_empty());
}

#[test]
fn resizing_with_nothing_focused_does_nothing() {
    let mut s = state(1);
    assert!(s.apply(&Command::ResizeActive(10, 10)).is_empty());
    assert!(s.apply(&Command::MoveWindow(Direction::Left)).is_empty());
    assert!(s.apply(&Command::ToggleFloating).is_empty());
    assert!(s.apply(&Command::Fullscreen).is_empty());
}

#[test]
fn a_tiled_window_swaps_with_its_neighbour() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.apply(&Command::MoveWindow(Direction::Left));
    assert_eq!(placed(&effects, X), Some(Rect::new(0, 0, 500, 960)));
    assert_eq!(placed(&effects, W), Some(Rect::new(500, 0, 500, 960)));
}

#[test]
fn moving_a_window_past_the_last_monitor_does_nothing() {
    let mut s = state(1);
    open(&mut s, W);
    assert!(s.apply(&Command::MoveWindow(Direction::Right)).is_empty());
}

#[test]
fn a_floating_window_tiles_again() {
    let mut s = state(1);
    open(&mut s, W);
    float(&mut s, X);
    assert!(s.is_floating(X));
    let effects = s.apply(&Command::ToggleFloating);
    assert!(!s.is_floating(X));
    assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
    assert!(effects.contains(&Effect::Focus(X)));
}

#[test]
fn fullscreen_toggles_off() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    s.apply(&Command::Fullscreen);
    let effects = s.apply(&Command::Fullscreen);
    assert_eq!(placed(&effects, X), Some(Rect::new(500, 0, 500, 960)));
    assert!(!s.snapshot_windows().iter().any(|w| w.fullscreen));
}

#[test]
fn focus_moves_between_floating_windows() {
    let mut s = state(1);
    float(&mut s, W);
    float(&mut s, X);
    for _ in 0..10 {
        s.apply(&Command::MoveWindow(Direction::Right));
    }
    assert_eq!(s.focused(), Some(X));
    s.apply(&Command::MoveFocus(Direction::Left));
    assert_eq!(s.focused(), Some(W));
}

#[test]
fn focus_starts_from_a_window_when_none_is_focused() {
    let mut s = state(1);
    open(&mut s, W);
    s.apply(&Command::Workspace(2));
    s.apply(&Command::Workspace(1));
    s.window_focused(Y);
    assert_eq!(s.focused(), None);
    let effects = s.apply(&Command::MoveFocus(Direction::Left));
    assert!(effects.contains(&Effect::Focus(W)));
}

#[test]
fn cycling_an_empty_workspace_does_nothing() {
    let mut s = state(1);
    assert!(s.apply(&Command::CycleNext).is_empty());
    assert!(s.apply(&Command::CyclePrev).is_empty());
}

#[test]
fn cycling_with_nothing_focused_starts_at_the_first() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    s.window_focused(Y);
    assert!(s.apply(&Command::CycleNext).contains(&Effect::Focus(W)));
}

#[test]
fn moving_to_the_current_workspace_does_nothing() {
    let mut s = state(1);
    open(&mut s, W);
    assert!(s.apply(&Command::MoveToWorkspace(1)).is_empty());
    let mut empty = state(1);
    assert!(empty.apply(&Command::MoveToWorkspace(2)).is_empty());
}

#[test]
fn moving_the_only_window_away_silently_leaves_the_workspace_empty() {
    let mut s = state(1);
    open(&mut s, W);
    let effects = s.apply(&Command::MoveToWorkspaceSilent(2));
    assert!(effects.contains(&Effect::Hide(W)));
    assert_eq!(s.focused(), None);
}

#[test]
fn layouts_switch_and_cycle() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    s.apply(&Command::Layout(LayoutKind::Monocle));
    assert_eq!(s.active_layout(), Some(LayoutKind::Monocle));
    let effects = s.sync();
    assert_eq!(placed(&effects, W), placed(&effects, X));
    s.apply(&Command::CycleLayout);
    assert_eq!(s.active_layout(), Some(LayoutKind::Scrolling));
    assert_eq!(s.snapshot_windows().len(), 2);
}

#[test]
fn layout_commands_ignore_other_layouts() {
    let mut s = state(1);
    open(&mut s, W);
    s.apply(&Command::Layout(LayoutKind::Monocle));
    for command in [
        Command::CycleProportion,
        Command::ScrollerProportion(0.5),
        Command::ExpelWindow,
        Command::CenterColumn,
        Command::ConsumeWindow(Direction::Left),
        Command::ToggleSplit,
        Command::SwapWithMaster,
    ] {
        assert!(s.apply(&command).is_empty(), "{command:?}");
    }
}

#[test]
fn scrolling_commands_change_the_columns() {
    let mut s = state(1);
    s.apply(&Command::Layout(LayoutKind::Scrolling));
    open(&mut s, W);
    open(&mut s, X);
    let before = s.sync();
    let after = s.apply(&Command::ScrollerProportion(1.0));
    assert_ne!(placed(&before, X), placed(&after, X));
    assert!(!s.apply(&Command::CycleProportion).is_empty());
    assert!(!s.apply(&Command::CenterColumn).is_empty());
    assert!(!s.apply(&Command::ConsumeWindow(Direction::Left)).is_empty());
    assert!(!s.apply(&Command::ExpelWindow).is_empty());
}

#[test]
fn dwindle_commands_rearrange() {
    let mut s = state(1);
    open(&mut s, W);
    open(&mut s, X);
    let before = placed(&s.sync(), X);
    let effects = s.apply(&Command::ToggleSplit);
    assert_ne!(placed(&effects, X), before);
    let effects = s.apply(&Command::SwapWithMaster);
    assert!(placed(&effects, W).is_some());
}

#[test]
fn master_swap_puts_the_focused_window_first() {
    let mut s = state(1);
    s.apply(&Command::Layout(LayoutKind::Master));
    open(&mut s, W);
    open(&mut s, X);
    let effects = s.apply(&Command::SwapWithMaster);
    assert!(placed(&effects, X).unwrap().x < placed(&effects, W).unwrap().x);
}

#[test]
fn settings_changes_reflow_and_info_updates() {
    let mut s = state(1);
    open(&mut s, W);
    let settings = LayoutSettings {
        gaps_in: 0,
        gaps_out: 20,
        ..LayoutSettings::default()
    };
    let effects = s.set_settings(settings, LayoutKind::Dwindle);
    assert_eq!(placed(&effects, W), Some(Rect::new(20, 20, 960, 920)));
    s.set_info(
        W,
        WindowInfo {
            title: "t".into(),
            class: "c".into(),
            exe: "e.exe".into(),
        },
    );
    let snap = s.snapshot_active_window().unwrap();
    assert_eq!((snap.title.as_str(), snap.exe.as_str()), ("t", "e.exe"));
    s.set_info(Y, WindowInfo::default());
}

#[test]
fn snapshots_describe_the_desktop() {
    let mut s = state(2);
    open(&mut s, W);
    s.apply(&Command::Workspace(2));
    open(&mut s, X);
    let windows = s.snapshot_windows();
    assert_eq!(windows.len(), 2);
    assert!(windows.windows(2).all(|p| p[0].workspace <= p[1].workspace));
    assert_eq!(s.snapshot_active_window().map(|w| w.id), Some(X.0));
    assert!(windows.iter().any(|w| w.id == W.0 && w.hidden));
    let workspaces = s.snapshot_workspaces();
    assert!(
        workspaces
            .iter()
            .any(|w| w.id == 2 && w.focused && w.visible)
    );
    assert!(workspaces.iter().any(|w| w.id == 1 && !w.visible));
    assert_eq!(s.snapshot_monitors().len(), 2);
    assert_eq!(s.hidden().count(), 1);
}

#[test]
fn reopening_a_monitor_restores_its_workspaces() {
    let mut s = state(2);
    s.apply(&Command::FocusMonitor(Direction::Right));
    open(&mut s, W);
    s.set_monitors(vec![monitor(1, 0)]);
    s.set_monitors(vec![monitor(1, 0), monitor(2, 1000)]);
    assert_eq!(s.monitor_count(), 2);
    assert!(s.is_managed(W));
}
