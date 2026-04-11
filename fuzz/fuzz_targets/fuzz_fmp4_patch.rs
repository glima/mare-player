#![no_main]

use libfuzzer_sys::fuzz_target;

use cosmic_applet_mare::audio::decoder::AudioDecoder;

fuzz_target!(|data: &[u8]| {
    // patch_fmp4_duration must never panic on arbitrary byte sequences,
    // regardless of duration or sample rate values.
    let mut buf = data.to_vec();
    AudioDecoder::patch_fmp4_duration(&mut buf, 10.0, 44100);

    // Also exercise edge-case numeric inputs.
    let mut buf2 = data.to_vec();
    AudioDecoder::patch_fmp4_duration(&mut buf2, 0.0, 0);

    let mut buf3 = data.to_vec();
    AudioDecoder::patch_fmp4_duration(&mut buf3, f64::MAX, u32::MAX);
});
