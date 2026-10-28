use crate::{FileSample, SettleWatcher, is_settled};

const A: FileSample = FileSample { size: 1000, mtime_ms: 10 };
const B: FileSample = FileSample { size: 2000, mtime_ms: 20 };
const EMPTY: FileSample = FileSample { size: 0, mtime_ms: 30 };

rl_test! {
    fn is_settled_needs_identical_non_empty_tail_samples() {
        assert!(!is_settled(&[], 3));
        assert!(!is_settled(&[Some(A), Some(A)], 3));
        assert!(is_settled(&[Some(A), Some(A), Some(A)], 3));
        assert!(is_settled(&[Some(B), Some(A), Some(A), Some(A)], 3));
        assert!(!is_settled(&[Some(A), Some(A), Some(B)], 3));
        assert!(!is_settled(&[Some(A), None, Some(A)], 3));
        assert!(!is_settled(&[Some(EMPTY), Some(EMPTY), Some(EMPTY)], 3));
        assert!(!is_settled(&[None, None, None], 3));
        // A threshold below 2 is clamped to 2.
        assert!(!is_settled(&[Some(A)], 0));
        assert!(is_settled(&[Some(A), Some(A)], 1));
    }
}

rl_test! {
    fn watcher_fires_once_per_settled_version() {
        let mut w = SettleWatcher::new(3);
        assert!(!w.observe(Some(A)));
        assert!(!w.observe(Some(A)));
        assert!(w.observe(Some(A)), "third identical sample settles");
        assert!(!w.observe(Some(A)), "already reported");
        assert!(!w.observe(Some(B)), "a change re-arms");
        assert!(!w.observe(Some(B)));
        assert!(w.observe(Some(B)));
    }
}

rl_test! {
    fn watcher_resets_on_missing_or_empty_files() {
        let mut w = SettleWatcher::default();
        assert!(!w.observe(Some(A)));
        assert!(!w.observe(Some(A)));
        assert!(!w.observe(None), "missing resets");
        assert!(!w.observe(Some(A)));
        assert!(!w.observe(Some(EMPTY)), "empty resets");
        assert!(!w.observe(Some(EMPTY)));
        assert!(!w.observe(Some(EMPTY)), "an empty file never settles");
        assert!(!w.observe(Some(A)));
        assert!(!w.observe(Some(A)));
        assert!(w.observe(Some(A)));
    }
}
