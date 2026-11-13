use super::*;

#[test]
fn public_window_bounds_preserve_empty_and_high_water_semantics() {
    let cases = [
        (
            Window::Page {
                after: u64::MAX,
                limit: 200,
            },
            100,
            Ok((100, 100)),
        ),
        (
            Window::Page {
                after: 99,
                limit: 200,
            },
            100,
            Ok((99, 100)),
        ),
        (
            Window::Range {
                after: u64::MAX,
                through: u64::MAX,
                limit: 1,
            },
            0,
            Ok((u64::MAX, u64::MAX)),
        ),
        (
            Window::Range {
                after: 99,
                through: u64::MAX,
                limit: 1,
            },
            100,
            Ok((99, 100)),
        ),
        (
            Window::Range {
                after: 99,
                through: 101,
                limit: 2,
            },
            100,
            Err(SandboxStoreError::SequenceConflict),
        ),
        (Window::Prefix { through: 0 }, 0, Ok((0, 0))),
        (
            Window::Prefix { through: 1 },
            0,
            Err(SandboxStoreError::SequenceConflict),
        ),
    ];
    for (window, head, expected) in cases {
        window.validate().unwrap();
        assert_eq!(window.bounds(head), expected);
    }
    for window in [
        Window::Page { after: 0, limit: 0 },
        Window::Page {
            after: 0,
            limit: MAX_PAGE_SIZE + 1,
        },
        Window::Range {
            after: 2,
            through: 1,
            limit: 1,
        },
    ] {
        assert_eq!(window.validate(), Err(SandboxStoreError::InvalidPage));
    }
}
