//! The remap layers of `examples/mbwilding.yml` (a Dvorak home-row-mods setup), driven through
//! the engine.

use std::path::Path;

use tile_remap::{Engine, Key, KeyEvent, Out};

fn engine() -> Engine {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mbwilding.yml");
    Engine::new(tile_config::Loaded::load(&path).unwrap().remap)
}

/// Feeds `events` (`"+a@0"` presses `a` at 0ms, `"-a@50"` releases it, `"tick@200"`
/// advances the clock) and returns what came out: `+k`/`-k` sent, `=+k`/`=-k` let through,
/// `guard` for the modifier guard.
fn run(engine: &mut Engine, events: &str) -> String {
    fn record(log: &mut Vec<String>, out: Vec<Out>) {
        log.extend(out.into_iter().map(|o| match o {
            Out::Press(k) => format!("+{k}"),
            Out::Release(k) => format!("-{k}"),
            Out::Guard => "guard".to_owned(),
        }));
    }
    let mut log = Vec::new();
    for event in events.split_whitespace() {
        let (what, time) = event.split_once('@').expect("event@time");
        let time: u64 = time.parse().unwrap();
        if what == "tick" {
            record(&mut log, engine.tick(time));
            continue;
        }
        let (down, name) = what.split_at(1);
        let key = Key::from_name(name).unwrap_or_else(|| panic!("unknown key {name}"));
        let verdict = engine.event(KeyEvent {
            key,
            down: down == "+",
            time,
        });
        if verdict.pass {
            assert!(verdict.out.is_empty());
            log.push(format!("={down}{key}"));
        }
        record(&mut log, verdict.out);
    }
    log.join(" ")
}

#[test]
fn unbound_keys_pass_through() {
    assert_eq!(run(&mut engine(), "+x@0 -x@10"), "=+x =-x");
}

#[test]
fn rolling_home_row_mods_types_letters() {
    assert_eq!(run(&mut engine(), "+a@0 +s@10 -a@50 -s@80"), "+a -a +s -s");
}

#[test]
fn held_home_row_mod_is_control() {
    let e = &mut engine();
    assert_eq!(
        run(e, "+a@0 tick@200 +j@250 -j@260 -a@300"),
        "+leftcontrol +j -j -leftcontrol"
    );
    // A key arriving after the timeout decides it even without a tick.
    assert_eq!(
        run(e, "+a@1000 +j@1250 -j@1260 -a@1300"),
        "+leftcontrol +j -j -leftcontrol"
    );
}

#[test]
fn keys_typed_during_the_timeout_are_replayed_in_order() {
    // `x` goes down while `a` is pending, then `a` turns out to be held.
    assert_eq!(
        run(&mut engine(), "+a@0 +x@50 tick@200 -x@210 -a@220"),
        "+leftcontrol +x -x -leftcontrol"
    );
}

#[test]
fn capslock_taps_backspace_and_holds_arrows() {
    let e = &mut engine();
    assert_eq!(
        run(e, "+capslock@0 -capslock@50"),
        "+leftcontrol -leftcontrol +backspace -backspace"
    );
    assert_eq!(
        run(e, "+capslock@100 +j@150 -j@160 -capslock@200"),
        "+leftcontrol -leftcontrol +left -left +leftcontrol -leftcontrol"
    );
    // Keys the layer doesn't bind get its Control.
    assert_eq!(
        run(e, "+capslock@300 +x@350 -x@360 -capslock@400"),
        "+leftcontrol +x -x -leftcontrol"
    );
}

#[test]
fn capslock_with_space_held_moves_by_word() {
    assert_eq!(
        run(
            &mut engine(),
            "+space@0 +capslock@10 +j@20 -j@30 -capslock@40 -space@50"
        ),
        "+leftshift +leftcontrol -leftshift +left -left +leftshift -leftcontrol -leftshift"
    );
}

#[test]
fn space_taps_space() {
    assert_eq!(
        run(&mut engine(), "+space@0 -space@50"),
        "+leftshift -leftshift +space -space"
    );
}

#[test]
fn number_layer() {
    assert_eq!(run(&mut engine(), "+f@0 +m@300 -m@310 -f@400"), "+1 -1");
}

#[test]
fn simple_remaps() {
    assert_eq!(
        run(
            &mut engine(),
            "+leftshift@0 -leftshift@10 +backspace@20 -backspace@30"
        ),
        "+esc -esc"
    );
}

#[test]
fn win_alone_is_guarded() {
    assert_eq!(
        run(&mut engine(), "+s@0 tick@200 -s@500"),
        "+leftmeta guard -leftmeta"
    );
    assert_eq!(
        run(&mut engine(), "+s@0 tick@200 +j@300 -j@310 -s@500"),
        "+leftmeta +j -j -leftmeta"
    );
}

#[test]
fn auto_repeat() {
    assert_eq!(
        run(
            &mut engine(),
            "+capslock@0 +j@10 +j@40 +j@70 -j@80 -capslock@90"
        ),
        "+leftcontrol -leftcontrol +left +left +left -left +leftcontrol -leftcontrol"
    );
    // A pending key's repeats are swallowed.
    assert_eq!(run(&mut engine(), "+a@0 +a@30 +a@60 -a@90"), "+a -a");
}

#[test]
fn reset_releases_everything() {
    let e = &mut engine();
    run(e, "+capslock@0 +j@10");
    assert_eq!(e.reset(), [Out::Release(Key::from_name("left").unwrap())]);
    // Its keys' releases then pass.
    assert_eq!(run(e, "-j@20 -capslock@30"), "=-j =-capslock");
    run(e, "+space@100");
    assert_eq!(
        e.reset(),
        [Out::Release(Key::from_name("leftshift").unwrap())]
    );
}

#[test]
fn control_and_meta_home_row_mods_combine() {
    // Hold `a` (Control) and `s` (Win), then press `j`.
    assert_eq!(
        run(
            &mut engine(),
            "+a@0 tick@200 +s@210 tick@410 +j@450 -j@460 -s@500 -a@510"
        ),
        "+leftcontrol +leftmeta +j -j -leftmeta -leftcontrol"
    );
}
