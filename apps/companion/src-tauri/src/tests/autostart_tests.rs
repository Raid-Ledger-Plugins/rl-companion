use crate::autostart::{
    first_run_action, launched_by_autostart, should_show_window, FirstRun, AUTOSTART_ARG,
};

const WIN: &str = r"C:\RL Companion\rl-companion.exe";
const MAC: &str = "/Applications/RL Companion.app/Contents/MacOS/rl-companion";

#[test]
fn autostart_arg_is_detected_anywhere_in_argv() {
    assert_eq!(AUTOSTART_ARG, "--autostart");
    assert!(launched_by_autostart([WIN, "--autostart"]));
    assert!(launched_by_autostart([MAC, "--other", "--autostart"]));
    assert!(!launched_by_autostart([WIN]));
    assert!(!launched_by_autostart(Vec::<String>::new()));
    // Only the exact flag counts.
    assert!(!launched_by_autostart([MAC, "--autostart=1"]));
    assert!(!launched_by_autostart([MAC, "autostart"]));
}

#[test]
fn a_login_launch_stays_hidden_and_a_manual_launch_shows_the_window() {
    assert!(!should_show_window([MAC, AUTOSTART_ARG]));
    assert!(should_show_window([MAC]));
    assert!(should_show_window([WIN]));
}

#[test]
fn a_forwarded_second_launch_shows_the_window_unless_it_is_a_login_launch() {
    // The single-instance callback gets the second process's argv as owned
    // strings, program path first, and passes `&argv`.
    let login: Vec<String> = vec![WIN.to_owned(), AUTOSTART_ARG.to_owned()];
    let manual: Vec<String> = vec![WIN.to_owned()];
    assert!(!should_show_window(&login));
    assert!(should_show_window(&manual));
}

#[test]
fn start_at_login_is_turned_on_only_on_the_first_run() {
    assert_eq!(first_run_action(false, false), FirstRun::EnableAndMark);
    assert_eq!(first_run_action(false, true), FirstRun::MarkOnly);
    // Once decided, a user's "off" sticks and an "on" is left as is.
    assert_eq!(first_run_action(true, false), FirstRun::Nothing);
    assert_eq!(first_run_action(true, true), FirstRun::Nothing);
}
