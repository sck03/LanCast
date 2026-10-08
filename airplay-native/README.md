# Optional AirPlay adapter

This is a separate GPL-3.0-only Cargo workspace. The Apache-2.0 control core does
not depend on it. Only the Android `airplay` receiver flavor links its JNI library.

- `src/engine.rs`: host admission, connection ownership, encoded media delivery.
- `src/identity.rs`: receiver signing and bounded peer trust.
- `src/avc.rs`: checked H.264 configuration, dimensions and Annex B conversion.
- `src/android.rs`: copying JNI boundary and integer engine handles.
- `vendor/rairplay`: reviewed fork of commit `7a0ec4036905afe0c8de16881d85d5846dc938a7`.
  `UPSTREAM.json` records original source hashes and the exact PlayFair submodule.

The fork adds real identity signing to legacy pairing, signature-failure rejection,
per-connection pairing selection, bounded RTSP framing, interruptible connections,
host approval before SETUP, explicit codec capabilities, NTP/RTP presentation clocks,
and media authentication/length checks. It does not advertise PTP, HLS, HEVC,
buffered music or multi-room playback. Authentication failure never selects a lower mode.

`cargo test --manifest-path airplay-native/Cargo.toml --workspace --locked` runs
protocol and adapter checks. This is not physical iPhone/Android TV certification.
See `docs/20-AirPlay实现与审阅指南.md` for the product and verification boundaries.
