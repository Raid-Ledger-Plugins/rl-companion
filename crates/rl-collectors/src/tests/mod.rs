//! Tests run natively (`cargo test`) and as WASM (`wasm-pack test --node`).
//! Fixtures are compiled in with `include_*!`, so no test touches the
//! filesystem. No real save data: packages are built by the test-only
//! writer from the contract's anonymised fixtures.

/// Declare a test that runs under `cargo test` and under wasm-bindgen-test.
macro_rules! rl_test {
    ($(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
        fn $name() $body
    };
}

mod bg3_loca_tests;
mod bg3_lsx_tests;
mod bg3_stats_tests;
mod extract_tests;
mod fixtures;
mod lsf_fixture_tests;
mod lsf_frame_tests;
mod lsf_tests;
mod lsf_writer;
mod lspk_parts_tests;
mod lspk_tests;
mod newage_tests;
mod normalise_tests;
mod real_zstd;
mod robustness_tests;
#[cfg(not(target_arch = "wasm32"))]
mod schema_tests;
mod settle_tests;
mod vdf_tests;
mod wow_sv_dev_tests;
mod wow_sv_paths_tests;
mod wow_sv_robustness_tests;
mod wow_sv_tests;
mod writer;
