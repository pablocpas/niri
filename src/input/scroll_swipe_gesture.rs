//! Swipe gesture from scroll events.
//!
//! Tracks when to begin, update, and end a swipe gesture from pointer axis events, also whether
//! the gesture is vertical or horizontal. Necessary because libinput only provides touchpad swipe
//! gesture events for 3+ fingers.

#[derive(Debug)]
pub struct ScrollSwipeGesture {
    ongoing: bool,
    vertical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSwipeGestureAction {
    BeginUpdate,
    Update,
    End,
}

impl ScrollSwipeGesture {
    pub const fn new() -> Self {
        Self {
            ongoing: false,
            vertical: false,
        }
    }

    pub fn update(&mut self, dx: f64, dy: f64) -> ScrollSwipeGestureAction {
        if dx == 0. && dy == 0. {
            self.ongoing = false;
            ScrollSwipeGestureAction::End
        } else if !self.ongoing {
            self.ongoing = true;
            self.vertical = dy != 0.;
            ScrollSwipeGestureAction::BeginUpdate
        } else {
            ScrollSwipeGestureAction::Update
        }
    }

    pub fn reset(&mut self) -> bool {
        if self.ongoing {
            self.ongoing = false;
            true
        } else {
            false
        }
    }

    pub fn is_vertical(&self) -> bool {
        self.vertical
    }
}

impl Default for ScrollSwipeGesture {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollSwipeGestureAction {
    pub fn begin(self) -> bool {
        self == ScrollSwipeGestureAction::BeginUpdate
    }

    pub fn end(self) -> bool {
        self == ScrollSwipeGestureAction::End
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollSwipeGesture, ScrollSwipeGestureAction};

    #[test]
    fn starts_on_first_non_zero_event() {
        let mut gesture = ScrollSwipeGesture::new();

        let action = gesture.update(0., 5.);
        assert_eq!(action, ScrollSwipeGestureAction::BeginUpdate);
        assert!(gesture.is_vertical());
    }

    #[test]
    fn keeps_updating_until_zero_event() {
        let mut gesture = ScrollSwipeGesture::new();
        let _ = gesture.update(5., 0.);

        assert_eq!(gesture.update(4., 0.), ScrollSwipeGestureAction::Update);
        assert_eq!(gesture.update(0., 0.), ScrollSwipeGestureAction::End);
    }

    #[test]
    fn reset_reports_whether_gesture_was_active() {
        let mut gesture = ScrollSwipeGesture::new();

        assert!(!gesture.reset());
        let _ = gesture.update(1., 0.);
        assert!(gesture.reset());
        assert!(!gesture.reset());
    }
}
