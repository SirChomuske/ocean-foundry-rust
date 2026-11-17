//! Stage 6 of the column bake plan: with a tiny memory limit, a large job
//! is refused or stopped while it has used only a few MB.
//!
//! A test file of its own, so it runs in its own process: the limit is
//! the whole process's.

use of_core::memory::{self, MB};
use of_core::params::{find, ParamValues, Value};
use of_formats::{catalogue, Progress};
use of_ocean::{bake_with, OceanSettings, Tuning};

#[test]
fn a_tiny_limit_refuses_and_stops_jobs_using_only_a_few_mb() {
    let start = memory::committed();
    memory::set_limit_mb(start / MB + 4);

    // The largest plot: refused by its estimate before anything is made.
    let mut values = ParamValues::default();
    for id in ["ocean.width", "ocean.length"] {
        values.set(find(id).unwrap(), Value::Int(2048));
    }
    let settings = OceanSettings::from_params(&values);
    let more = of_ocean::estimate::job(&settings, None);
    assert!(more > 500 * MB, "{more}");
    assert!(memory::fits(more).is_err());

    // Run anyway, it is stopped by the guard as soon as it reports.
    let refused = bake_with("big", &settings, None, catalogue::default(), &Tuning::default(), &Progress::none());
    assert_eq!(refused.err().as_deref(), Some(of_formats::MEMORY));

    // A build file that would unpack to 400 MB is stopped while unpacking.
    let mut bomb = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..400 {
        std::io::Write::write_all(&mut bomb, &zeros).unwrap();
    }
    let bytes = bomb.finish().unwrap();
    let read = of_formats::import(&bytes, &Progress::none());
    assert_eq!(read.err().map(|e| e.key), Some(of_formats::MEMORY));

    let used = memory::committed().saturating_sub(start);
    assert!(used < 16 * MB, "used {} MB", used / MB);
}
