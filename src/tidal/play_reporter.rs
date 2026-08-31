// SPDX-License-Identifier: GPL-3.0-only

//! Report track plays to TIDAL's Event Producer bus so they count for the
//! user's "Recently Played", recommendations, and artist royalty accounting.
//!
//! Without this module, plays through mare-player never tell TIDAL the track
//! was played: `tidlers` fetches stream URLs but doesn't wrap the
//! event-producer endpoint.  This module talks to that endpoint directly,
//! in the wire format TIDAL's iOS/Android/web SDKs use.
//!
//! ## Wire format
//!
//! `POST https://ec.tidal.com/api/event-batch` with **AWS SQS
//! `SendMessageBatch` form encoding** (not JSON for the envelope).  Each
//! entry carries a JSON `MessageBody` with the actual `playback_session`
//! event, plus a `Headers` MessageAttribute with identity metadata.
//!
//! ## Body shape: mimic a mobile client (not Web)
//!
//! The downstream `play_log` consumer routes events into Recently
//! Played only when the body looks like it came from a mobile client.
//! Both the iOS and Android SDKs include `user` and `client` objects
//! inside the `MessageBody`; the Web SDK is the outlier and its
//! events get filtered out of Recently Played.  The platform / device
//! type / app version we claim come from
//! [`client_identity`](super::client_identity), so they describe the
//! same TIDAL client the access token was minted for; the numeric
//! client id and session id are read out of the token itself.
//!
//! ## Recently Played vs aggregate counters
//!
//! `sourceType` / `sourceId` are the key knob:
//! * `ALBUM` / `PLAYLIST` / `MIX` / `ARTIST` container events surface in
//!   Recently Played and credit royalties.
//! * `TRACK` events count toward "Most Listened" aggregates but **don't**
//!   surface in Recently Played.
//!
//! mare-player currently doesn't thread container source through the
//! playback layer end-to-end, so we fall back to `TRACK`/`track_id` for
//! the bare-track case until that plumbing is in place.
//!
//! ## Threading model
//!
//! All sends happen on a background tokio task fed by an unbounded mpsc
//! channel.  Playback never waits on the network — `record()` is
//! non-blocking and never errors.  A rolling buffer of the most recent
//! attempts is kept in memory and exposed via [`PlayReporter::recent_log`]
//! so a future diagnostic UI can surface the round-trip status.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose};
use serde::Deserialize;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::client_identity::TIDAL_CLIENT;

// ── Constants ──────────────────────────────────────────────────────────

const EVENT_URL: &str = "https://ec.tidal.com/api/event-batch";
const EVENT_NAME: &str = "playback_session";
const EVENT_GROUP: &str = "play_log";
const EVENT_VERSION: u32 = 2;

const CONSENT_CATEGORY: &str = "NECESSARY";

/// Maximum number of recent report attempts kept in the in-memory
/// diagnostic log.  Older entries fall off when this cap is hit.
const REPORT_LOG_CAP: usize = 50;

// ── Public types ───────────────────────────────────────────────────────

/// One completed track listen, ready to be reported to TIDAL.
///
/// `end_position_s - start_position_s` is the actual listened duration
/// (i.e. skipped-over seeks don't inflate it).  TIDAL reconstructs the
/// listen using `actions[]` plus the start/end timestamps.
///
/// `access_token` is snapshotted at record time so the worker doesn't have
/// to reach back into the auth manager from a background tokio task.
#[derive(Debug, Clone)]
pub struct PlaySession {
    pub session_id: String,
    pub track_id: String,
    /// e.g. `"HI_RES_LOSSLESS"`, `"LOSSLESS"`, `"HIGH"`, `"LOW"`.
    pub quality: String,
    /// Container that started this listen: `"ALBUM"`, `"PLAYLIST"`,
    /// `"MIX"`, `"ARTIST"`, or `"TRACK"` when no container context.
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub start_ts_ms: u64,
    pub end_ts_ms: u64,
    pub start_position_s: f64,
    pub end_position_s: f64,
    /// TIDAL access token, snapshotted at record time.  The worker
    /// extracts the `uid`/`cid`/`sid` claims for attribution and sends
    /// it back as the `authorization` Header MessageAttribute.
    pub access_token: String,
}

/// One row of the in-memory diagnostic log.
#[derive(Debug, Clone)]
pub struct ReportLogEntry {
    pub ts_ms: u64,
    pub phase: &'static str,
    pub track_id: String,
    pub http_status: Option<u16>,
    pub note: String,
}

/// Background worker that POSTs `PlaySession`s to TIDAL's event bus.
///
/// Construct once at app startup, share `Arc<PlayReporter>` between the
/// playback handler and (optionally) a diagnostic UI view.
#[derive(Debug)]
pub struct PlayReporter {
    tx: UnboundedSender<PlaySession>,
    log: Arc<Mutex<VecDeque<ReportLogEntry>>>,
}

impl PlayReporter {
    /// Spawn the background worker.
    pub fn spawn() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let log: Arc<Mutex<VecDeque<ReportLogEntry>>> = Arc::new(Mutex::new(VecDeque::with_capacity(REPORT_LOG_CAP)));
        let worker_log = Arc::clone(&log);

        tokio::spawn(async move {
            run_worker(rx, worker_log).await;
        });

        Arc::new(Self { tx, log })
    }

    /// Queue a session for reporting.  Never blocks; never errors.
    pub fn record(&self, session: PlaySession) {
        // Channel is unbounded; only fails if the worker thread has
        // panicked, in which case we just drop the event.
        let _ = self.tx.send(session);
    }

    /// Snapshot of the rolling diagnostic log (most-recent last).
    pub fn recent_log(&self) -> Vec<ReportLogEntry> {
        match self.log.lock() {
            Ok(g) => g.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}

// ── In-progress session tracking ────────────────────────────────────

/// Bookkeeping for the currently-playing track's session.
///
/// The playback handler opens one of these when a track starts, updates
/// `last_position_s` on every tick, and turns it into a [`PlaySession`]
/// via [`InProgressPlay::finalize`] when the track ends.
#[derive(Debug, Clone)]
pub struct InProgressPlay {
    pub session_id: String,
    pub track_id: String,
    pub quality: String,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub start_ts_ms: u64,
    pub start_position_s: f64,
    /// High-water mark of the position seen during this listen.  Track
    /// position can drop briefly at end-of-track when the engine resets
    /// before the next-track transition; using the max preserves the
    /// real listened duration.
    pub last_position_s: f64,
    /// Total track duration in seconds (for the 50%-listened threshold).
    pub duration_s: f64,
}

impl InProgressPlay {
    /// Open a new session for `track_id`.
    pub fn open(
        track_id: String,
        quality: String,
        source_type: Option<String>,
        source_id: Option<String>,
        start_position_s: f64,
        duration_s: f64,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            track_id,
            quality,
            source_type,
            source_id,
            start_ts_ms: now_ms(),
            start_position_s,
            last_position_s: start_position_s,
            duration_s,
        }
    }

    /// Bump `last_position_s` to `pos_s` if it's higher than the current
    /// high-water mark.  Cheap to call on every playback tick.
    pub fn observe_position(&mut self, pos_s: f64) {
        if pos_s > self.last_position_s {
            self.last_position_s = pos_s;
        }
    }

    /// True if the listen exceeded 30 seconds OR 50% of the track
    /// duration, whichever comes first.  Matches the threshold Spotify
    /// and Apple Music use — short skips don't generate spurious plays.
    pub fn meets_threshold(&self) -> bool {
        let listened = (self.last_position_s - self.start_position_s).max(0.0);
        listened >= 30.0 || (self.duration_s > 0.0 && listened >= self.duration_s / 2.0)
    }

    /// Consume `self` and produce a finalized [`PlaySession`] ready to
    /// send.  Caller stamps the current timestamp + access token at the
    /// moment of finalisation.
    pub fn finalize(self, end_ts_ms: u64, access_token: String) -> PlaySession {
        PlaySession {
            session_id: self.session_id,
            track_id: self.track_id,
            quality: self.quality,
            source_type: self.source_type,
            source_id: self.source_id,
            start_ts_ms: self.start_ts_ms,
            end_ts_ms,
            start_position_s: self.start_position_s,
            end_position_s: self.last_position_s,
            access_token,
        }
    }
}

// ── Worker ─────────────────────────────────────────────────────────────

async fn run_worker(mut rx: UnboundedReceiver<PlaySession>, log: Arc<Mutex<VecDeque<ReportLogEntry>>>) {
    let http = reqwest::Client::new();
    debug!("play_reporter worker started");
    while let Some(session) = rx.recv().await {
        let token = session.access_token.clone();
        if token.is_empty() {
            push_log(
                &log,
                ReportLogEntry {
                    ts_ms: now_ms(),
                    phase: "skipped",
                    track_id: session.track_id.clone(),
                    http_status: None,
                    note: "empty access token".to_string(),
                },
            );
            continue;
        }

        let claims = decode_jwt_claims(&token);
        let body = build_message_body(&session, &claims);
        let headers_attr = build_headers_attr(&token, claims.cid.as_deref());
        let form = encode_sqs_batch(&Uuid::new_v4().to_string(), &body, &headers_attr);
        let form_body = match serde_urlencoded::to_string(&form) {
            Ok(b) => b,
            Err(e) => {
                warn!("play-report form encoding failed: {e}");
                continue;
            }
        };

        let entry = match http
            .post(EVENT_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Authorization", format!("Bearer {}", token))
            .body(form_body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let note = if status >= 400 {
                    let body_text = resp.text().await.unwrap_or_default();
                    let snip: String = body_text.chars().take(300).collect();
                    format!("error body: {snip}")
                } else {
                    let src_label = match (&session.source_type, &session.source_id) {
                        (Some(t), Some(i)) => format!("{t}:{i}"),
                        _ => "none".to_string(),
                    };
                    format!(
                        "cid={} user={} src={}",
                        claims.cid.as_deref().unwrap_or("?"),
                        claims.uid.as_deref().unwrap_or("?"),
                        src_label,
                    )
                };
                ReportLogEntry {
                    ts_ms: now_ms(),
                    phase: "sent",
                    track_id: session.track_id.clone(),
                    http_status: Some(status),
                    note,
                }
            }
            Err(e) => {
                warn!("play-report send failed: {e}");
                ReportLogEntry {
                    ts_ms: now_ms(),
                    phase: "sent",
                    track_id: session.track_id.clone(),
                    http_status: None,
                    note: format!("network error: {e}"),
                }
            }
        };

        info!(
            phase = entry.phase,
            track = %entry.track_id,
            status = ?entry.http_status,
            note = %entry.note,
            "play-report"
        );
        push_log(&log, entry);
    }
    debug!("play_reporter worker exiting (channel closed)");
}

// ── Body / headers / form encoding ─────────────────────────────────────

fn build_message_body(session: &PlaySession, claims: &JwtClaims) -> String {
    // sourceType / sourceId are typed as non-nullable strings in TIDAL's
    // schema (`playback-session.ts`); empty string is the "unset" value.
    // Sending `null` fails validation silently and the event is dropped.
    let source_type = session.source_type.as_deref().unwrap_or("");
    let source_id = session.source_id.as_deref().unwrap_or("");

    let user_id_int: u64 = claims.uid.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
    let client_id_int: u64 = claims.cid.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
    let session_id = claims.sid.as_deref().unwrap_or("");

    let payload = serde_json::json!({
        "playbackSessionId": session.session_id,
        "productType": "TRACK",
        "actualProductId": session.track_id,
        "requestedProductId": session.track_id,
        "actualAssetPresentation": "FULL",
        "actualAudioMode": "STEREO",
        "actualQuality": session.quality,
        "sourceType": source_type,
        "sourceId": source_id,
        "startTimestamp": session.start_ts_ms,
        "endTimestamp": session.end_ts_ms,
        "startAssetPosition": session.start_position_s,
        "endAssetPosition": session.end_position_s,
        "isPostPaywall": true,
        "actions": [
            {
                "actionType": "PLAYBACK_START",
                "assetPosition": session.start_position_s,
                "timestamp": session.start_ts_ms,
            },
            {
                "actionType": "PLAYBACK_STOP",
                "assetPosition": session.end_position_s,
                "timestamp": session.end_ts_ms,
            },
        ],
    });

    let envelope = serde_json::json!({
        "group": EVENT_GROUP,
        "version": EVENT_VERSION,
        "ts": now_ms(),
        "uuid": Uuid::new_v4().to_string(),
        "user": {
            "id": user_id_int,
            "clientId": client_id_int,
            "sessionId": session_id,
        },
        "client": {
            "token": client_id_int.to_string(),
            "deviceType": TIDAL_CLIENT.device_type,
            "version": TIDAL_CLIENT.app_version,
            "platform": TIDAL_CLIENT.platform,
        },
        "payload": payload,
        "extras": serde_json::Value::Null,
    });

    serde_json::to_string(&envelope).unwrap_or_default()
}

fn build_headers_attr(token: &str, client_id: Option<&str>) -> String {
    // The Headers MessageAttribute travels alongside the MessageBody in
    // every SQS entry.  These are the keys TIDAL's mobile SDKs send, which
    // is what we authenticate as.
    //
    // `authorization` is the raw token, NO "Bearer " prefix — that's only
    // on the outer HTTP Authorization header.
    let headers = serde_json::json!({
        "app-name": TIDAL_CLIENT.app_name,
        "app-version": TIDAL_CLIENT.app_version,
        "client-id": client_id.unwrap_or("unknown"),
        "consent-category": CONSENT_CATEGORY,
        "os-name": TIDAL_CLIENT.platform,
        "requested-sent-timestamp": now_ms(),
        "authorization": token,
    });
    serde_json::to_string(&headers).unwrap_or_default()
}

fn encode_sqs_batch(msg_id: &str, body: &str, headers_attr: &str) -> Vec<(String, String)> {
    // AWS SQS `SendMessageBatch` form-encoded parameters.  We only ever
    // send one entry at a time; the indexing convention is 1-based.
    vec![
        ("SendMessageBatchRequestEntry.1.Id".to_string(), msg_id.to_string()),
        ("SendMessageBatchRequestEntry.1.MessageBody".to_string(), body.to_string()),
        ("SendMessageBatchRequestEntry.1.MessageAttribute.1.Name".to_string(), "Name".to_string()),
        ("SendMessageBatchRequestEntry.1.MessageAttribute.1.Value.StringValue".to_string(), EVENT_NAME.to_string()),
        ("SendMessageBatchRequestEntry.1.MessageAttribute.1.Value.DataType".to_string(), "String".to_string()),
        ("SendMessageBatchRequestEntry.1.MessageAttribute.2.Name".to_string(), "Headers".to_string()),
        ("SendMessageBatchRequestEntry.1.MessageAttribute.2.Value.StringValue".to_string(), headers_attr.to_string()),
        ("SendMessageBatchRequestEntry.1.MessageAttribute.2.Value.DataType".to_string(), "String".to_string()),
    ]
}

// ── JWT claim decoding ─────────────────────────────────────────────────

/// Subset of TIDAL JWT claims we read.  Attribution is entirely
/// server-side from `uid` (user id) and `cid` (numeric client id);
/// `sid` is echoed back as the session id mobile clients carry.
#[derive(Debug, Default, Deserialize)]
struct JwtClaims {
    /// User id, sometimes called `sub` in standards-compliant tokens.
    uid: Option<String>,
    /// Numeric client id of the OAuth/PKCE client the token was issued to.
    cid: Option<String>,
    /// Session id (mobile-style); empty for desktop clients.
    sid: Option<String>,
}

/// Decode a JWT payload (best-effort).  Failures yield `Default` so the
/// caller never has to handle errors — a play with missing claims still
/// gets sent, it just won't attribute to a user/client in TIDAL's logs.
fn decode_jwt_claims(token: &str) -> JwtClaims {
    let payload_b64 = match token.split('.').nth(1) {
        Some(p) => p,
        None => return JwtClaims::default(),
    };
    // JWTs are base64url without padding; restore the padding so we can
    // use the URL_SAFE_NO_PAD decoder either way.
    let mut padded = payload_b64.to_string();
    let pad = (4 - padded.len() % 4) % 4;
    padded.extend(std::iter::repeat_n('=', pad));

    let bytes = match general_purpose::URL_SAFE.decode(&padded) {
        Ok(b) => b,
        Err(_) => return JwtClaims::default(),
    };

    // TIDAL serialises `cid` and `uid` as integers, not strings.  Parse
    // into a permissive Value first and coerce to String so callers get a
    // single representation regardless of whether TIDAL ever switches.
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return JwtClaims::default(),
    };

    fn coerce(v: Option<&serde_json::Value>) -> Option<String> {
        v.and_then(|x| match x {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    }

    JwtClaims {
        uid: coerce(value.get("uid")).or_else(|| coerce(value.get("sub"))),
        cid: coerce(value.get("cid")),
        sid: coerce(value.get("sid")),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn push_log(log: &Arc<Mutex<VecDeque<ReportLogEntry>>>, entry: ReportLogEntry) {
    if let Ok(mut g) = log.lock() {
        if g.len() >= REPORT_LOG_CAP {
            g.pop_front();
        }
        g.push_back(entry);
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_claims_parses_uid_cid_sid() {
        // Synthetic JWT (no signature check):
        //   header   = {"alg":"none"}
        //   payload  = {"uid":12345,"cid":8017,"sid":"sess-abc"}
        let header = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"uid":12345,"cid":8017,"sid":"sess-abc"}"#);
        let token = format!("{header}.{payload}.");
        let claims = decode_jwt_claims(&token);
        assert_eq!(claims.uid.as_deref(), Some("12345"));
        assert_eq!(claims.cid.as_deref(), Some("8017"));
        assert_eq!(claims.sid.as_deref(), Some("sess-abc"));
    }

    #[test]
    fn jwt_claims_falls_back_to_sub() {
        let header = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"42"}"#);
        let token = format!("{header}.{payload}.");
        let claims = decode_jwt_claims(&token);
        assert_eq!(claims.uid.as_deref(), Some("42"));
    }

    #[test]
    fn jwt_claims_yields_default_on_garbage() {
        let claims = decode_jwt_claims("not a token");
        assert!(claims.uid.is_none());
        assert!(claims.cid.is_none());
    }

    #[test]
    fn message_body_contains_required_fields() {
        let session = PlaySession {
            session_id: "sess-1".to_string(),
            track_id: "12345".to_string(),
            quality: "LOSSLESS".to_string(),
            source_type: Some("ALBUM".to_string()),
            source_id: Some("999".to_string()),
            start_ts_ms: 1_700_000_000_000,
            end_ts_ms: 1_700_000_180_000,
            start_position_s: 0.0,
            end_position_s: 180.0,
            access_token: String::new(),
        };
        let claims = JwtClaims { uid: Some("42".to_string()), cid: Some("8017".to_string()), sid: Some("sess-abc".to_string()) };
        let body = build_message_body(&session, &claims);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["group"], "play_log");
        assert_eq!(v["version"], 2);
        assert_eq!(v["payload"]["actualProductId"], "12345");
        assert_eq!(v["payload"]["sourceType"], "ALBUM");
        assert_eq!(v["payload"]["sourceId"], "999");
        assert_eq!(v["payload"]["actualQuality"], "LOSSLESS");
        assert_eq!(v["user"]["id"], 42);
        assert_eq!(v["user"]["clientId"], 8017);
        assert_eq!(v["client"]["platform"], TIDAL_CLIENT.platform);
        assert_eq!(v["client"]["deviceType"], TIDAL_CLIENT.device_type);
        // PLAYBACK_START + PLAYBACK_STOP.
        assert_eq!(v["payload"]["actions"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn headers_attr_omits_bearer_prefix() {
        let attr = build_headers_attr("raw-token", Some("8017"));
        let v: serde_json::Value = serde_json::from_str(&attr).unwrap();
        assert_eq!(v["authorization"], "raw-token");
        assert_eq!(v["client-id"], "8017");
        assert_eq!(v["app-name"], TIDAL_CLIENT.app_name);
        assert_eq!(v["os-name"], TIDAL_CLIENT.platform);
    }

    #[test]
    fn empty_source_falls_back_to_empty_string_not_null() {
        // Sending `null` for sourceType/sourceId fails the SDK schema
        // validation silently; empty string is the documented unset value.
        let session = PlaySession {
            session_id: "x".to_string(),
            track_id: "1".to_string(),
            quality: "LOW".to_string(),
            source_type: None,
            source_id: None,
            start_ts_ms: 0,
            end_ts_ms: 1000,
            start_position_s: 0.0,
            end_position_s: 1.0,
            access_token: String::new(),
        };
        let body = build_message_body(&session, &JwtClaims::default());
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["payload"]["sourceType"], "");
        assert_eq!(v["payload"]["sourceId"], "");
    }

    #[test]
    fn in_progress_threshold_30s_listen() {
        let mut p = InProgressPlay::open("42".into(), "LOSSLESS".into(), None, None, 0.0, 300.0);
        assert!(!p.meets_threshold());
        p.observe_position(29.5);
        assert!(!p.meets_threshold());
        p.observe_position(30.0);
        assert!(p.meets_threshold());
    }

    #[test]
    fn in_progress_threshold_half_duration() {
        // Short track: 50% = 10s, less than the 30s cutoff.
        let mut p = InProgressPlay::open("42".into(), "LOSSLESS".into(), None, None, 0.0, 20.0);
        p.observe_position(9.0);
        assert!(!p.meets_threshold());
        p.observe_position(10.0);
        assert!(p.meets_threshold());
    }

    #[test]
    fn in_progress_observe_is_high_water_mark() {
        // Engine may briefly snap position back to 0 right before a track
        // switch.  observe_position must not regress.
        let mut p = InProgressPlay::open("42".into(), "LOSSLESS".into(), None, None, 0.0, 300.0);
        p.observe_position(200.0);
        p.observe_position(0.0);
        assert_eq!(p.last_position_s, 200.0);
    }

    #[test]
    fn sqs_batch_has_single_1based_entry_with_both_attributes() {
        let params = encode_sqs_batch("msg-7", "the-body", "the-headers");
        let map: std::collections::HashMap<_, _> = params.into_iter().collect();

        assert_eq!(map["SendMessageBatchRequestEntry.1.Id"], "msg-7");
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageBody"], "the-body");
        // Attribute 1 = Name -> EVENT_NAME
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageAttribute.1.Name"], "Name");
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageAttribute.1.Value.StringValue"], EVENT_NAME);
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageAttribute.1.Value.DataType"], "String");
        // Attribute 2 = Headers -> headers_attr
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageAttribute.2.Name"], "Headers");
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageAttribute.2.Value.StringValue"], "the-headers");
        assert_eq!(map["SendMessageBatchRequestEntry.1.MessageAttribute.2.Value.DataType"], "String");
    }
}
