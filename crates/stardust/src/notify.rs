use notify_rust::{Notification, Timeout};
use stardust_core::policy::{Presentation, BUBBLE_VISIBLE_FOR};

pub fn show(presentation: &Presentation) {
    let millis = u32::try_from(BUBBLE_VISIBLE_FOR.as_millis()).unwrap_or(6_000);
    if let Err(err) = Notification::new()
        .summary("Stardust")
        .body(&presentation.summary.body())
        .timeout(Timeout::Milliseconds(millis))
        .show()
    {
        eprintln!("stardust: could not show an OS notification: {err}");
    }
}
