//! Screen control: `screen_screenshot` and `screen_action` work
//! only while the person has turned on "Let Wysebee see and control this Mac's screen" — off by
//! default, forgotten on unpairing — through the screen the shell hands in (a fake here).

mod common;

use agent_core::agent::{PAUSED, SCREEN_OFF};
use agent_core::screen::{Action, Button};
use agent_core::{Agent, Step};
use common::{call, fake, pair, Bench};
use serde_json::json;

#[tokio::test]
async fn nothing_is_seen_or_done_until_the_person_turns_it_on() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;
    assert!(!agent.status().screen_control);
    assert_eq!(fake.last_status()["screen_control"], false);
    for (id, op, args) in [("c1", "screen_screenshot", json!({})), ("c2", "screen_action", json!({"action": "click", "x": 1, "y": 1}))] {
        assert_eq!(call(&agent, &fake, id, op, args).await, json!({"ok": false, "error": SCREEN_OFF}));
    }
    assert!(bench.screen.actions.lock().unwrap().is_empty(), "nothing was done");

    agent.set_screen_control(true);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(fake.last_status()["screen_control"], true);
    assert!(agent.status().screen_control);

    // A screenshot: the picture, scaled to fit, the pointer where it is.
    let shot = call(&agent, &fake, "c3", "screen_screenshot", json!({})).await;
    let r = &shot["result"];
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["screen_width"].as_u64(), r["mime"].as_str()), (Some(1430), Some(804), Some(1600), Some("image/jpeg")));
    assert_eq!(r["cursor"], json!([715, 402]));
    assert!(r["image"].as_str().unwrap().len() > 100);

    // An action: in the picture's pixels, done in the screen's, then the new screenshot.
    let done = call(&agent, &fake, "c4", "screen_action", json!({"action": "click", "x": 715, "y": 402})).await;
    assert_eq!(done["result"]["done"], "Clicked at 800, 450 on the screen.");
    assert_eq!(done["result"]["width"], 1430);
    // Asked for no screenshot: none.
    let typed = call(&agent, &fake, "c5", "screen_action", json!({"action": "type", "text": "hunter2", "screenshot": false})).await;
    assert_eq!(typed, json!({"ok": true, "result": {"done": "Typed 7 characters."}}));
    assert_eq!(*bench.screen.actions.lock().unwrap(), vec![Action::Click { x: 800, y: 450, button: Button::Left, count: 1 }, Action::Type { text: "hunter2".into() }]);
    // An action that cannot be read does nothing, and says why.
    let wrong = call(&agent, &fake, "c6", "screen_action", json!({"action": "click", "x": 5000, "y": 1})).await;
    assert!(wrong["error"].as_str().unwrap().contains("outside the screenshot"), "{wrong}");
    assert_eq!(bench.screen.actions.lock().unwrap().len(), 2);
    // The screen failing: said.
    *bench.screen.failing.lock().unwrap() = Some("The display is asleep.".into());
    assert_eq!(call(&agent, &fake, "c7", "screen_screenshot", json!({})).await, json!({"ok": false, "error": "The display is asleep."}));
    *bench.screen.failing.lock().unwrap() = None;

    // Paused: refused.
    agent.set_paused(true).await;
    assert_eq!(call(&agent, &fake, "c8", "screen_screenshot", json!({})).await, json!({"ok": false, "error": PAUSED}));
    agent.set_paused(false).await;
    // Off again: refused again; on, then unpaired: forgotten.
    agent.set_screen_control(false);
    assert_eq!(call(&agent, &fake, "c9", "screen_screenshot", json!({})).await, json!({"ok": false, "error": SCREEN_OFF}));
    agent.set_screen_control(true);
    agent.unpair().await;
    assert!(!agent.state().screen_control && !agent.status().screen_control);
}

#[tokio::test]
async fn a_pc_with_no_screen_to_show_says_so() {
    let (fake, bench) = (fake().await, Bench::new());
    let mut options = bench.options(Some(&fake.url), Bench::timing());
    options.screen = None;
    let agent = Agent::open(options);
    pair(&agent, &fake).await;
    agent.set_screen_control(true);
    let none = call(&agent, &fake, "c1", "screen_screenshot", json!({})).await;
    assert_eq!(none, json!({"ok": false, "error": "This Mac has no screen Wysebee Agent can show."}));
}
