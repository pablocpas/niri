#[derive(Debug)]
pub struct ScrollTracker {
    tick: f64,
    last: f64,
    acc: f64,
}

impl ScrollTracker {
    pub fn new(threshold: i32) -> Self {
        Self {
            tick: f64::from(threshold.max(1)),
            last: 0.,
            acc: 0.,
        }
    }

    pub fn accumulate(&mut self, amount: f64) -> i32 {
        let changed_direction = (self.last > 0. && amount < 0.) || (self.last < 0. && amount > 0.);
        if changed_direction {
            self.acc = 0.;
        }

        self.last = amount;
        self.acc += amount;

        let mut ticks = 0;
        if self.acc.abs() >= self.tick {
            let clamped = self.acc.clamp(-127. * self.tick, 127. * self.tick);
            ticks = (clamped / self.tick).trunc() as i32;
            self.acc %= self.tick;
        }

        ticks
    }

    pub fn reset(&mut self) {
        self.last = 0.;
        self.acc = 0.;
    }
}

#[cfg(test)]
mod tests {
    use super::ScrollTracker;

    #[test]
    fn accumulate_emits_discrete_ticks() {
        let mut tracker = ScrollTracker::new(10);

        assert_eq!(tracker.accumulate(3.), 0);
        assert_eq!(tracker.accumulate(4.), 0);
        assert_eq!(tracker.accumulate(5.), 1);
        assert_eq!(tracker.accumulate(8.), 1);
    }

    #[test]
    fn accumulate_handles_negative_direction() {
        let mut tracker = ScrollTracker::new(10);

        assert_eq!(tracker.accumulate(-4.), 0);
        assert_eq!(tracker.accumulate(-9.), -1);
        assert_eq!(tracker.accumulate(-8.), -1);
    }

    #[test]
    fn accumulate_resets_when_direction_changes() {
        let mut tracker = ScrollTracker::new(10);

        assert_eq!(tracker.accumulate(9.), 0);
        assert_eq!(tracker.accumulate(-2.), 0);
        assert_eq!(tracker.accumulate(-8.), -1);
    }

    #[test]
    fn accumulate_clamps_huge_bursts() {
        let mut tracker = ScrollTracker::new(10);

        assert_eq!(tracker.accumulate(100000.), 127);
        assert_eq!(tracker.accumulate(-100000.), -127);
    }

    #[test]
    fn reset_clears_remainder() {
        let mut tracker = ScrollTracker::new(10);

        assert_eq!(tracker.accumulate(8.), 0);
        tracker.reset();
        assert_eq!(tracker.accumulate(8.), 0);
        assert_eq!(tracker.accumulate(2.), 1);
    }
}
