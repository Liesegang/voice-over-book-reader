#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CounterPolicy {
    pub(crate) surfaces: usize,
    pub(crate) capacity: usize,
}

impl CounterPolicy {
    pub(crate) fn choose(
        cpus: Option<usize>,
        value: Option<&str>,
        data_dir: bool,
        perf_output: bool,
    ) -> Self {
        let surfaces = if data_dir && perf_output {
            match value {
                Some("1") => 1,
                Some("2") => 2,
                _ => usize::from(cpus.is_some_and(|count| count >= 8)) + 1,
            }
        } else {
            usize::from(cpus.is_some_and(|count| count >= 8)) + 1
        };
        Self {
            surfaces,
            capacity: if surfaces == 2 { 2 } else { 3 },
        }
    }

    pub(crate) fn needed(self, queued: usize, in_flight: usize) -> usize {
        queued
            .saturating_add(in_flight)
            .div_ceil(self.capacity)
            .min(self.surfaces)
    }

    pub(crate) fn can_start_second(self, first_ready: bool, first_in_flight: usize) -> bool {
        self.surfaces > 1 && first_ready && first_in_flight > 0
    }
}

#[cfg(test)]
mod tests {
    use super::CounterPolicy;

    const SINGLE: CounterPolicy = CounterPolicy {
        surfaces: 1,
        capacity: 3,
    };
    const DOUBLE: CounterPolicy = CounterPolicy {
        surfaces: 2,
        capacity: 2,
    };

    #[test]
    fn cpu_threshold_and_unavailable_cpu_count_choose_bounded_defaults() {
        for cpus in [None, Some(0), Some(1), Some(2), Some(4), Some(7)] {
            assert_eq!(CounterPolicy::choose(cpus, None, false, false), SINGLE);
        }
        for cpus in [Some(8), Some(12), Some(64), Some(usize::MAX)] {
            assert_eq!(CounterPolicy::choose(cpus, None, false, false), DOUBLE);
        }
    }

    #[test]
    fn diagnostic_overrides_require_both_data_directory_and_perf_output() {
        for (data_dir, perf_output) in [(false, false), (false, true), (true, false)] {
            assert_eq!(
                CounterPolicy::choose(Some(8), Some("1"), data_dir, perf_output),
                DOUBLE
            );
            assert_eq!(
                CounterPolicy::choose(Some(4), Some("2"), data_dir, perf_output),
                SINGLE
            );
        }
        assert_eq!(
            CounterPolicy::choose(Some(8), Some("1"), true, true),
            SINGLE
        );
        assert_eq!(
            CounterPolicy::choose(Some(4), Some("2"), true, true),
            DOUBLE
        );
        assert_eq!(CounterPolicy::choose(None, Some("2"), true, true), DOUBLE);
    }

    #[test]
    fn unrecognized_diagnostic_values_preserve_the_cpu_default() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("3"),
            Some(" 2"),
            Some("invalid"),
        ] {
            assert_eq!(CounterPolicy::choose(Some(8), value, true, true), DOUBLE);
            assert_eq!(CounterPolicy::choose(Some(7), value, true, true), SINGLE);
            assert_eq!(CounterPolicy::choose(None, value, true, true), SINGLE);
        }
    }

    #[test]
    fn needed_surfaces_count_both_queued_and_assigned_work() {
        assert_eq!(DOUBLE.needed(0, 0), 0);
        assert_eq!(DOUBLE.needed(1, 0), 1);
        assert_eq!(DOUBLE.needed(0, 2), 1);
        assert_eq!(DOUBLE.needed(1, 2), 2);
        assert_eq!(DOUBLE.needed(4, 0), 2);
        assert_eq!(DOUBLE.needed(100, 4), 2);
        assert_eq!(SINGLE.needed(0, 0), 0);
        assert_eq!(SINGLE.needed(0, 3), 1);
        assert_eq!(SINGLE.needed(100, 3), 1);
    }

    #[test]
    fn needed_surfaces_remain_bounded_when_work_counts_overflow() {
        assert_eq!(DOUBLE.needed(usize::MAX, usize::MAX), 2);
        assert_eq!(SINGLE.needed(usize::MAX, 1), 1);
    }

    #[test]
    fn second_surface_waits_for_a_ready_first_surface_with_assigned_work() {
        assert!(!DOUBLE.can_start_second(false, 0));
        assert!(!DOUBLE.can_start_second(false, 2));
        assert!(!DOUBLE.can_start_second(true, 0));
        assert!(DOUBLE.can_start_second(true, 1));
        assert!(DOUBLE.can_start_second(true, 2));
        for first_ready in [false, true] {
            for first_in_flight in [0, 1, 3] {
                assert!(!SINGLE.can_start_second(first_ready, first_in_flight));
            }
        }
    }
}
