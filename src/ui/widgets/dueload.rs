//! Due-load histogram: braille bars of Task counts per upcoming day ("workload
//! ahead"), derived from cached due dates. Rides in the task pane's title bar.

/// `counts[0]` = due today, `counts[1]` = +1 day, ... Rendered as a braille strip.
pub fn render(_counts: &[usize], _ascii: bool) -> String {
    todo!("scale counts to braille bar heights; ASCII fallback")
}
