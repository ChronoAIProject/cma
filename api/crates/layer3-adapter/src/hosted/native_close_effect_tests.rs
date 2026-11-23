use super::*;
use crate::owner_protocol::{scope, test_support::TestAuthority};
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::field::{Field, Visit};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{layer::Context, prelude::*, Layer};

const RECEIVER_TEST: &str = "link::tests::agent_resources::close_effect::runtime_receiver";
const FENCE: u64 = 7;

fn timestamp_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros()
}

#[derive(Default)]
struct CheckoutFields {
    control: bool,
    fresh: bool,
}

impl Visit for CheckoutFields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "lane" {
            self.control = format!("{value:?}") == "Control";
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "reused_connection" {
            self.fresh = !value;
        }
    }
}

#[derive(Clone, Default)]
struct CloseCheckout(Arc<OnceLock<u128>>);

impl<S: tracing::Subscriber> Layer<S> for CloseCheckout {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut fields = CheckoutFields::default();
        event.record(&mut fields);
        if fields.control && fields.fresh {
            self.0
                .set(timestamp_us())
                .expect("one fresh close checkout");
        }
    }
}

async fn line(control: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    assert_ne!(
        control.read_line(&mut line).await.unwrap(),
        0,
        "runtime fixture exited"
    );
    line
}

async fn stage(control: &mut BufReader<TcpStream>, name: &str) -> u128 {
    let event = line(control).await;
    let (stage, micros) = event.trim_end().split_once(' ').unwrap();
    assert_eq!(stage, name);
    micros.parse().unwrap()
}

async fn exercise(receiver: &std::ffi::OsStr, mode: NativeRuntimeTransport) {
    let coordination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut runtime = tokio::process::Command::new(receiver)
        .args(["--exact", RECEIVER_TEST, "--ignored", "--nocapture"])
        .env(
            "CMA_NATIVE_CLOSE_COORDINATION",
            coordination.local_addr().unwrap().to_string(),
        )
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (control, _) = tokio::select! {
        accepted = coordination.accept() => accepted.unwrap(),
        status = runtime.wait() => panic!("runtime failed before connecting: {status:?}"),
    };
    let mut control = BufReader::new(control);
    let address: SocketAddr = line(&mut control).await.trim_end().parse().unwrap();
    assert!(address.ip().is_loopback());
    let fixture = Fixture::start(Mode::Forward(address), mode).await;
    let endpoint = fixture.endpoint().await;
    control
        .get_mut()
        .write_all(format!("{}\n", endpoint.control_token().unwrap()).as_bytes())
        .await
        .unwrap();

    let handle = handle();
    // Give Close a second slot using a separate pool state with the same
    // capacity authority and Pod binding. Both exchanges use the normal
    // control route, independently of any proposed CloseAgent lane.
    let mut closer = (*fixture.transport).clone();
    closer.state = Arc::new(Mutex::new(TransportState::default()));
    let ordinary = fixture.transport.exchange(&handle, RequestFrame::Resources);
    tokio::pin!(ordinary);
    let held_at = tokio::select! {
        held = stage(&mut control, "held") => held,
        result = &mut ordinary => panic!("ordinary response escaped the runtime hold: {result:?}"),
    };
    assert_eq!(fixture.state.active.load(Ordering::SeqCst), 1);

    let close = crate::AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    let request = RequestFrame::CloseAgent(FencedRequest::new(FENCE, close.clone()));
    let checkpoint = CloseCheckout::default();
    let close_exchange = scope(TestAuthority::new(FENCE), closer.exchange(&handle, request))
        .with_subscriber(tracing_subscriber::registry().with(checkpoint.clone()));
    let (wire_at, effect_at, ack_at, response) = tokio::time::timeout(Duration::from_secs(8), async {
        let events = async {
            let wire = stage(&mut control, "wire").await;
            eprintln!("{mode:?} wire_us={}", wire - held_at);
            let effect = stage(&mut control, "effect").await;
            (wire, effect)
        };
        let acknowledgement = async {
            let response = close_exchange.await.unwrap();
            (timestamp_us(), response)
        };
        tokio::select! {
            result = &mut ordinary => panic!("ordinary response escaped the runtime hold: {result:?}"),
            ((wire, effect), (ack, response)) = async { tokio::join!(events, acknowledgement) } => {
                (wire, effect, ack, response)
            }
        }
    })
    .await
    .expect("second connection must complete CloseAgent with Resources still held");
    let checkout_at = *checkpoint.0.get().expect("fresh Control checkout");
    let ResponseFrame::CloseAgent(receipt) = response else {
        panic!("expected successful fenced CloseAgent receipt, received {response:?}");
    };
    assert_eq!(receipt.authority_fence, FENCE);
    assert_eq!(receipt.receipt.operation_id, close.operation_id);
    assert_eq!(receipt.receipt.request_sha256, close.request_sha256);
    assert_eq!(receipt.receipt.agent_id, close.agent_id);
    assert!(
        held_at <= checkout_at
            && checkout_at <= wire_at
            && wire_at <= effect_at
            && effect_at <= ack_at
    );
    eprintln!(
        "{mode:?} checkout_us={} wire_us={} effect_us={} acknowledgement_us={}",
        checkout_at - held_at,
        wire_at - held_at,
        effect_at - held_at,
        ack_at - held_at,
    );

    control.get_mut().write_all(b"release\n").await.unwrap();
    assert!(matches!(
        ordinary.await.unwrap(),
        ResponseFrame::Resources(_)
    ));
    control.get_mut().write_all(b"verify\n").await.unwrap();
    assert!(stage(&mut control, "closed").await >= ack_at);
    assert!(runtime.wait().await.unwrap().success());
    eprintln!("{mode:?} late Resources response left the Agent closed");
}

#[tokio::test]
#[ignore = "requires the Layer 4 test binary via CMA_NATIVE_CLOSE_RECEIVER"]
async fn authorized_close_takes_effect_while_ordinary_response_is_held() {
    let receiver = std::env::var_os("CMA_NATIVE_CLOSE_RECEIVER")
        .expect("set CMA_NATIVE_CLOSE_RECEIVER to the built layer4-runtime test binary");
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        tokio::time::timeout(Duration::from_secs(20), exercise(&receiver, mode))
            .await
            .unwrap_or_else(|_| panic!("{mode:?} runtime effect control timed out"));
    }
}
