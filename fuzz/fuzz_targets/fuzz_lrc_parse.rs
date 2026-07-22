#![no_main]

use libfuzzer_sys::fuzz_target;

use cosmic_applet_mare::tidal::models::parse_lrc;

fuzz_target!(|data: &str| {
    // parse_lrc turns untrusted LRC/subtitle text from the TIDAL API into
    // timed lyric lines. It must never panic on arbitrary input.
    let _ = parse_lrc(data);
});
