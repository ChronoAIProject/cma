use super::*;

#[test]
fn failure_retains_last_good_batch_but_never_reports_it_as_fresh_success() {
    let cache = Cache::default();
    assert!(cache.render().is_empty());
    cache.initialize();
    assert!(!cache.render().contains("cma_task_cohort_accepted_tasks{"));
    cache.update(Ok(
        "cma_task_cohort_accepted_tasks{window=\"release\"} 7\n".into()
    ));
    let before = cache.render();
    cache.update(Err("budget"));
    let after = cache.render();
    assert!(after.contains("cma_task_cohort_sample_success{window=\"release\"} 0"));
    assert!(after.contains("reason=\"budget\"} 1"));
    assert!(after.contains("cma_task_cohort_accepted_tasks{window=\"release\"} 7"));
    let success_at = |text: &str| {
        text.lines()
            .find(|line| line.starts_with("cma_task_cohort_sample_success_timestamp_seconds{"))
            .unwrap()
            .to_owned()
    };
    assert_eq!(success_at(&before), success_at(&after));
    let restarted = Cache::default();
    restarted.initialize();
    assert!(!restarted
        .render()
        .contains("cma_task_cohort_accepted_tasks{"));
}

#[test]
fn concurrent_scrapes_never_mix_fields_from_two_cohort_batches() {
    let cache = Cache::default();
    let writer = cache.clone();
    let task = std::thread::spawn(move || {
        for n in 0..2_000 {
            writer.update(Ok(format!("batch_a {n}\nbatch_b {n}\n")));
        }
    });
    for _ in 0..2_000 {
        let text = cache.render();
        let fields: Vec<_> = text
            .lines()
            .filter(|line| line.starts_with("batch_"))
            .collect();
        if !fields.is_empty() {
            assert_eq!(
                fields[0].split_whitespace().nth(1),
                fields[1].split_whitespace().nth(1)
            );
        }
    }
    task.join().unwrap();
}
