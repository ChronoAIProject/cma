use super::*;

pub(super) fn pipeline(report_id: &str) -> [Document; 8] {
    [
        doc! {"$match":{"report_id":report_id}},
        doc! {"$project":{"_id":1,"sandbox_id":1,"owner":1}},
        doc! {"$lookup":{"from":super::super::sandbox_usage::STATES,"localField":"sandbox_id","foreignField":"_id","as":"current"}},
        doc! {"$lookup":{"from":"sandboxes","localField":"sandbox_id","foreignField":"_id","as":"sandbox"}},
        // Project after both equality joins to preserve both indexed SBE lookups.
        // Keep the Sandbox array: [{}] must still differ from a deleted Sandbox [].
        doc! {"$project":{"_id":1,"owner":1,"current.owner":1,"sandbox.owner":1}},
        doc! {"$match":{"$expr":{"$or":[
            {"$ne":["$owner",{"$arrayElemAt":["$current.owner",0]}]},
            {"$and":[{"$gt":[{"$size":"$sandbox"},0]},
                {"$ne":["$owner",{"$arrayElemAt":["$sandbox.owner",0]}]}]}
        ]}}},
        doc! {"$limit":1},
        doc! {"$project":{"_id":1}},
    ]
}
