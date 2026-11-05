//! State shared by the tray, the windows, and the check timer: the latest
//! check, the monitor scale, and, with the newer features on, the ORBIT
//! connection and the requests the check timer carries out on its next tick.

use stardust_core::connect::FlowPhase;
use stardust_core::delivery::{DeliveryBoard, Outcome};
use stardust_core::pet::ObservedCheck;
use std::fmt;
use std::sync::{Arc, Mutex};

/// "Send test" requests waiting at once. More clicks are dropped.
const MAX_PENDING_TESTS: usize = 16;

#[derive(Clone)]
pub struct LivePet {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    observed: ObservedCheck,
    scale_factor: f64,
    phase: FlowPhase,
    signed_in: Option<bool>,
    connect_requested: bool,
    tests: Vec<String>,
    sign_out: Option<SignOutTicket>,
    delivery: DeliveryBoard,
}

/// A sign-out from the settings window, finished by the check timer: it
/// deletes the keychain record again in case a running check saved one, and
/// asks ORBIT to revoke the refresh token when there was one. Debug output
/// never shows the token.
pub struct SignOutTicket(Option<String>);

impl SignOutTicket {
    pub fn refresh_token(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl fmt::Debug for SignOutTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = self.0.as_ref().map(|_| "<redacted>");
        formatter
            .debug_tuple("SignOutTicket")
            .field(&shown)
            .finish()
    }
}

impl LivePet {
    pub fn new(scale_factor: f64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                observed: ObservedCheck::Waiting,
                scale_factor: finite_scale(scale_factor),
                phase: FlowPhase::Idle,
                signed_in: None,
                connect_requested: false,
                tests: Vec::new(),
                sign_out: None,
                delivery: DeliveryBoard::default(),
            })),
        }
    }

    pub fn observed(&self) -> ObservedCheck {
        self.lock().observed
    }

    pub fn set_observed(&self, observed: ObservedCheck) {
        self.lock().observed = observed;
    }

    pub fn scale_factor(&self) -> f64 {
        self.lock().scale_factor
    }

    pub fn set_scale_factor(&self, scale_factor: f64) {
        if scale_factor.is_finite() && scale_factor > 0.0 {
            self.lock().scale_factor = scale_factor;
        }
    }

    /// Where the device sign-in on the check timer stands.
    pub fn phase(&self) -> FlowPhase {
        self.lock().phase.clone()
    }

    pub fn set_phase(&self, phase: FlowPhase) {
        self.lock().phase = phase;
    }

    /// A token is saved, as far as the last sign-in, sign-out, or check
    /// knows. `None` until one of them, or the window at start, tells:
    /// without a window nothing reads the keychain just to fill this in.
    pub fn signed_in(&self) -> Option<bool> {
        self.lock().signed_in
    }

    pub fn set_signed_in(&self, signed_in: bool) {
        self.lock().signed_in = Some(signed_in);
    }

    /// Ask the check timer to start the device sign-in.
    pub fn request_connect(&self) {
        self.lock().connect_requested = true;
    }

    pub fn take_connect(&self) -> bool {
        std::mem::take(&mut self.lock().connect_requested)
    }

    /// Ask the check timer to post the test message to integration `id`.
    /// A second click before the first is handled does nothing.
    pub fn request_test(&self, id: &str) {
        let mut inner = self.lock();
        if inner.tests.len() < MAX_PENDING_TESTS && !inner.tests.iter().any(|queued| queued == id) {
            inner.tests.push(id.to_string());
        }
    }

    pub fn take_tests(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().tests)
    }

    /// Hand a sign-out to the check timer, with the refresh token to revoke
    /// if the record had one. The caller deletes the keychain copy first.
    pub fn queue_sign_out(&self, refresh_token: Option<&str>) {
        let refresh = refresh_token
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        self.lock().sign_out = Some(SignOutTicket(refresh));
    }

    pub fn take_sign_out(&self) -> Option<SignOutTicket> {
        self.lock().sign_out.take()
    }

    /// The last delivery result per integration, with the panel on.
    pub fn delivery(&self) -> DeliveryBoard {
        self.lock().delivery.clone()
    }

    pub fn record_delivery(&self, id: &str, outcome: Outcome) {
        self.lock().delivery.record(id, outcome);
    }

    /// Forget a result that no longer says anything, as when the user turns
    /// an integration back on after it was switched off.
    pub fn clear_delivery(&self, id: &str) {
        self.lock().delivery.clear(id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }
}

fn finite_scale(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::LivePet;

    #[test]
    fn requests_are_taken_once_and_tests_are_not_queued_twice() {
        let live = LivePet::new(1.0);
        assert!(!live.take_connect());
        live.request_connect();
        live.request_connect();
        assert!(live.take_connect());
        assert!(!live.take_connect());
        live.request_test("ops");
        live.request_test("ops");
        live.request_test("pd");
        assert_eq!(live.take_tests(), ["ops", "pd"]);
        assert!(live.take_tests().is_empty());
        for index in 0..40 {
            live.request_test(&format!("id-{index}"));
        }
        assert_eq!(live.take_tests().len(), super::MAX_PENDING_TESTS);
    }

    #[test]
    fn a_queued_sign_out_is_taken_once_and_hides_its_refresh_token() {
        let live = LivePet::new(1.0);
        assert!(live.take_sign_out().is_none());
        live.queue_sign_out(None);
        assert_eq!(live.take_sign_out().unwrap().refresh_token(), None);
        live.queue_sign_out(Some(""));
        assert_eq!(live.take_sign_out().unwrap().refresh_token(), None);
        live.queue_sign_out(Some("refresh-secret"));
        let ticket = live.take_sign_out().unwrap();
        assert_eq!(ticket.refresh_token(), Some("refresh-secret"));
        assert!(!format!("{ticket:?}").contains("refresh-secret"));
        assert!(live.take_sign_out().is_none());
    }

    #[test]
    fn delivery_results_are_kept_per_integration_until_cleared() {
        use stardust_core::delivery::Outcome;
        let live = LivePet::new(1.0);
        assert_eq!(live.delivery().get("ops"), None);
        live.record_delivery("ops", Outcome::RetryPending);
        live.clone().record_delivery("ops", Outcome::Delivered);
        assert_eq!(live.delivery().get("ops"), Some(Outcome::Delivered));
        live.clear_delivery("ops");
        assert_eq!(live.delivery().get("ops"), None);
    }
}
