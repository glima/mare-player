#![no_main]

use libfuzzer_sys::fuzz_target;

use cosmic_applet_mare::audio::dash::DashManifest;

fuzz_target!(|data: &str| {
    // DashManifest::parse must never panic on arbitrary input.
    let _ = DashManifest::parse(data);
});
