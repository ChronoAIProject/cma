use super::*;

#[test]
fn abort_custody_requires_an_unqualified_success_reply() {
    use mongodb::bson::{doc, Bson};
    for ok in [Bson::Int32(1), Bson::Int64(1), Bson::Double(1.0)] {
        assert!(acknowledged_abort_reply(&doc! {"ok":ok.clone()}));
        for field in ["writeConcernError", "errorLabels"] {
            let mut reply = doc! {"ok":ok.clone()};
            reply.insert(field, Bson::Null);
            assert!(!acknowledged_abort_reply(&reply));
        }
    }
    for reply in [
        doc! {},
        doc! {"ok":0},
        doc! {"ok":"1"},
        doc! {"ok":true},
        doc! {"ok":1.0,"errorLabels":[]},
        doc! {"ok":1.0,"writeConcernError":{"code":64}},
    ] {
        assert!(!acknowledged_abort_reply(&reply));
    }
}
