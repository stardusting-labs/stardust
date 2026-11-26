# Stardust voice cloud v1 draft

**Proposed, not served by ORBIT.** This is a Stardust client proposal. The ORBIT desktop contract at `rust-orbit` main `e8403a39` documents device sign-in, desktop status, and alerts and explicitly excludes chat. A server route and its provider require a separate ORBIT decision.

## User interaction

The intended desktop interaction is to click the pet, open its panel, then press and release a push-to-talk button for one English turn. The panel shows a text answer and its source citations. It displays “I don't know” when the response offers no usable citation, and “Voice is not available yet” when the service is unavailable. Capture starts only on an explicit press. The draft scope excludes wake-word listening, voice activity detection, background capture, speech playback, and on-device models.

The `stardust-core` state machine exposes `Idle`, `Recording`, `Sending`, `Answer`, `NotAvailable`, and `Error`. Release or the 10-second capture limit ends recording. The microphone adapter must enforce the same limit if the UI stops polling. The core accepts mono PCM16 from 8 to 192 kHz and resamples it to 16 kHz. Cancel discards recorded audio or cancels queued work. If a request has started, cancel discards its eventual result. The future panel binds press, release, and cancel to accessible controls and presents the state as text. The microphone adapter and panel wiring require their own authorization.

## Proposed request

- `POST /api/pqc/v1/desktop/voice/turn` on the configured ORBIT origin.
- Send the desktop access token in the `Authorization: Bearer` header over HTTPS. Plain HTTP is reserved for loopback development and tests. Redirects are disabled.
- Send `Content-Type: audio/wav` and `Accept: application/json`. The body is a canonical 44-byte RIFF/WAVE header followed by little-endian mono PCM16 data at 16,000 Hz. One turn contains 1 to 160,000 samples and at most 320,044 bytes. The server would also enforce the limit and derive workspace authorization from the desktop session.
- The client sends one request per submitted turn. The flag `STARDUST_VOICE_V1` enables it only for the exact value `1`.

## Proposed response

An HTTP `200` JSON answer uses this shape. Text is English and citations refer to evidence the signed-in user can access.

```json
{
  "kind": "answer",
  "transcript": "Did anything change?",
  "answer": "One new alert needs attention.",
  "citations": [
    { "label": "New alert", "workspace_path": "/rust-preview/pqc/studio/app/control-room/needs-attention" }
  ]
}
```

Citation labels are display text. Citation paths must pass the existing workspace path validation and resolve on the configured HTTPS origin. An empty citation list is accepted and results in “I don't know”. The client also falls back when the answer is blank, the list is missing, or every citation fails validation. Render transcript, answer, and labels as plain text.

An HTTP `200` JSON `{"kind":"not_available"}` represents an unavailable turn. A `404`, a refused connection, a timeout, or a temporary service failure also yields `NotAvailable`. `401` and `403` produce an authorization error. Other unexpected statuses and invalid JSON produce a response error. The JSON read is limited to 65,536 bytes, with a default client timeout of 10 seconds. The HTTP client cannot interrupt synchronous OS DNS resolution. The client returns static errors and keeps the token, audio, and server response text out of them.

## Evidence and handoff

Unit tests exercise the audio and state boundaries. Loopback HTTP tests exercise the proposed wire format and failure behavior. Provider authentication, evidence grounding, data retention, and production behavior require ORBIT review and integration evidence. Real microphone capture requires the owner’s crate card for this lane.
