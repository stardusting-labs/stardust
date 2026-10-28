//! Latest check and monitor scale shared by the tray and the pet window.

use stardust_core::pet::ObservedCheck;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct LivePet {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    observed: ObservedCheck,
    scale_factor: f64,
}

impl LivePet {
    pub fn new(scale_factor: f64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                observed: ObservedCheck::Waiting,
                scale_factor: finite_scale(scale_factor),
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
