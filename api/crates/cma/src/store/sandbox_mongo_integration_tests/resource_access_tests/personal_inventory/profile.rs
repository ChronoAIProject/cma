//! Local phase diagnostics on the same public router and disposable Mongo fixture.
use super::*;
use std::{collections::BTreeMap, time::Instant};
use tracing::{
    span::{Attributes, Id},
    Subscriber,
};
use tracing_subscriber::{
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
    Layer,
};

type Phases = BTreeMap<String, (u128, u128)>;
#[path = "profile/identity.rs"]
mod identity;
#[path = "profile/request.rs"]
mod request;
#[cfg(test)]
#[path = "profile/tests.rs"]
mod tests;

#[derive(Clone, Default)]
pub(super) struct Profile {
    phases: Arc<Mutex<Phases>>,
    request: Arc<Mutex<Option<request::Sample>>>,
}
#[derive(serde::Serialize)]
pub(super) struct Sample {
    phases: Phases,
    request: Option<request::Sample>,
}
struct SpanTime {
    started: Instant,
    entered: Option<Instant>,
    busy: u128,
}
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Profile {
    fn on_new_span(&self, _: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        ctx.span(id).unwrap().extensions_mut().insert(SpanTime {
            started: Instant::now(),
            entered: None,
            busy: 0,
        });
    }
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        ctx.span(id)
            .unwrap()
            .extensions_mut()
            .get_mut::<SpanTime>()
            .unwrap()
            .entered = Some(Instant::now());
    }
    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        let mut extensions = span.extensions_mut();
        let time = extensions.get_mut::<SpanTime>().unwrap();
        if let Some(start) = time.entered.take() {
            time.busy += start.elapsed().as_micros();
        }
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).unwrap();
        let extensions = span.extensions();
        let time = extensions.get::<SpanTime>().unwrap();
        let mut totals = self.phases.lock().unwrap();
        let entry = totals.entry(span.name().to_owned()).or_default();
        entry.0 += time.started.elapsed().as_micros();
        entry.1 += time.busy;
    }
}
impl Profile {
    pub fn dispatch(&self) -> tracing::Dispatch {
        tracing::Dispatch::new(
            tracing_subscriber::registry().with(self.clone().with_filter(
                tracing_subscriber::filter::filter_fn(|m| m.target() == "cma.agent_inventory"),
            )),
        )
    }
    pub fn measure<F: std::future::Future>(
        &self,
        future: F,
    ) -> impl std::future::Future<Output = F::Output> {
        request::Measured::new(future, self.request.clone())
    }
    pub fn take(&self) -> Sample {
        Sample {
            phases: std::mem::take(&mut *self.phases.lock().unwrap()),
            request: self.request.lock().unwrap().take(),
        }
    }
}
pub(super) fn report(requests: &[Sample]) {
    let mut grouped: BTreeMap<String, Vec<(u128, u128)>> = BTreeMap::new();
    for request in requests {
        let timing = request.request.as_ref().expect("profiled request timing");
        assert!(timing.completed && !timing.identity.invalid);
        assert_eq!(
            timing.identity.resolutions, 1,
            "one ambient identity resolution per request"
        );
        assert_eq!(
            timing.identity.resolved, 1,
            "successful identity resolution required"
        );
        for (name, times) in &request.phases {
            grouped.entry(name.clone()).or_default().push(*times);
        }
    }
    for (phase, mut times) in grouped {
        times.sort_unstable();
        let n = times.len();
        let mean = times.iter().map(|t| t.0).sum::<u128>() / n as u128;
        let busy = times.iter().map(|t| t.1).sum::<u128>() / n as u128;
        println!("AGENT_LIST_PHASE phase={phase} requests={n} mean_wall_us={mean} mean_active_us={busy} p95_wall_us={}",times[(n*95).div_ceil(100)-1].0);
    }
}
pub(super) fn record(path: &str, distinct: bool, index: usize, status: u16, sample: &Sample) {
    println!(
        "AGENT_LIST_REQUEST_TIMING {}",
        serde_json::json!({
            "schema":1, "path":path, "distinct":distinct, "sample":index,
            "status":status, "timing":sample
        })
    );
}
pub(super) async fn server(f: &fixture::Fixture, enable: bool) {
    if enable {
        f.client
            .database(&f.name)
            .create_collection("system.profile")
            .capped(true)
            .size(128 * 1024 * 1024)
            .await
            .unwrap();
    }
    f.client
        .database(&f.name)
        .run_command(doc! {"profile":if enable {2} else {0},"slowms":0})
        .await
        .unwrap();
}
// Count actual server commands as an independent check on connection checkouts.
// Profiling must not install the driver's eager full-reply BSON observer.
pub(super) async fn server_report(f: &fixture::Fixture) -> meter::Stats {
    let mut cursor = f
        .client
        .database(&f.name)
        .collection::<Document>("system.profile")
        .find(doc! {"appName":&f.app_name})
        .await
        .unwrap();
    let mut groups: BTreeMap<String, Vec<(i64, i64, i64)>> = BTreeMap::new();
    let mut counts = meter::Stats::default();
    while cursor.advance().await.unwrap() {
        let row: Document = cursor.deserialize_current().unwrap();
        let command = row.get_document("command").expect("profiled command");
        *counts
            .commands
            .entry(command_key(&row).unwrap())
            .or_default() += 1;
        counts.failed += usize::from(row.contains_key("errCode") || row.contains_key("exception"));
        let op = row.get_str("op").unwrap_or("none");
        let name = command
            .get_str("comment")
            .unwrap_or_else(|_| row.get_str("ns").unwrap_or(op));
        let integer = |field| {
            row.get_i64(field)
                .or_else(|_| row.get_i32(field).map(i64::from))
                .unwrap_or(0)
        };
        groups.entry(format!("{op}/{name}")).or_default().push((
            integer("millis"),
            integer("cpuNanos"),
            integer("docsExamined"),
        ));
    }
    for (phase, mut rows) in groups {
        rows.sort_unstable();
        let n = rows.len();
        println!("AGENT_LIST_SERVER phase={phase} commands={n} sum_ms={} p95_ms={} sum_cpu_ns={} docs_examined={}",
            rows.iter().map(|r|r.0).sum::<i64>(),rows[(n*95).div_ceil(100)-1].0,
            rows.iter().map(|r|r.1).sum::<i64>(),rows.iter().map(|r|r.2).sum::<i64>());
    }
    counts
}

fn command_key(row: &Document) -> Result<String, &'static str> {
    let command = row.get_document("command").map_err(|_| "missing command")?;
    // Mongo truncates large pipeline text in system.profile. Never use or log
    // that text as a metric label; these fixed comments identify our aggregates.
    if command.contains_key("$truncated") {
        if !matches!(row.get_str("op"), Ok("command"))
            || !row.get_str("ns").is_ok_and(|ns| ns.ends_with(".agents"))
            || !matches!(
                command.get_str("comment"),
                Ok("cma.personal_agent_inventory"
                    | "cma.personal_agent_evidence"
                    | "cma.personal_agent_selection")
            )
        {
            return Err("unrecognized truncated command metadata");
        }
        return Ok("agents/aggregate".into());
    }
    let (operation, value) = command.iter().next().ok_or("empty command")?;
    let collection = if operation == "getMore" {
        command
            .get_str("collection")
            .map_err(|_| "missing collection")?
    } else {
        value.as_str().ok_or("missing collection")?
    };
    Ok(format!("{collection}/{operation}"))
}

#[test]
fn profiler_counts_normal_and_truncated_commands_without_query_labels() {
    for (command, expected) in [
        (doc! {"find":"auth_subjects"}, "auth_subjects/find"),
        (
            doc! {"getMore":42_i64,"collection":"agents"},
            "agents/getMore",
        ),
        (doc! {"aggregate":"agents"}, "agents/aggregate"),
        (
            doc! {"$truncated":"private query text","comment":"cma.personal_agent_evidence"},
            "agents/aggregate",
        ),
    ] {
        let row = doc! {"command":command,"op":"command","ns":"test.agents"};
        assert_eq!(command_key(&row), Ok(expected.into()));
    }
    for row in [
        doc! {},
        doc! {"command":{}},
        doc! {"command":{"getMore":42_i64}},
        doc! {"command":{"$truncated":"private query text"},"op":"command","ns":"test.agents"},
        doc! {"command":{"$truncated":"private query text","comment":"cma.personal_agent_evidence"},"op":"query","ns":"test.agents"},
        doc! {"command":{"$truncated":"private query text","comment":"cma.personal_agent_evidence"},"op":"command","ns":"test.other"},
    ] {
        assert!(command_key(&row).is_err());
    }
}
