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
#[derive(Clone, Default)]
pub(super) struct Profile(Arc<Mutex<Phases>>);
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
        let mut totals = self.0.lock().unwrap();
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
    pub fn take(&self) -> Phases {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}
pub(super) fn report(requests: &[Phases]) {
    let mut grouped: BTreeMap<String, Vec<(u128, u128)>> = BTreeMap::new();
    for request in requests {
        for (name, times) in request {
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
pub(super) async fn server_report(f: &fixture::Fixture) {
    let mut cursor = f
        .client
        .database(&f.name)
        .collection::<Document>("system.profile")
        .find(doc! {"appName":&f.app_name})
        .await
        .unwrap();
    let mut groups: BTreeMap<String, Vec<(i64, i64, i64)>> = BTreeMap::new();
    while cursor.advance().await.unwrap() {
        let row: Document = cursor.deserialize_current().unwrap();
        let Some(command) = row.get_document("command").ok() else {
            continue;
        };
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
}
