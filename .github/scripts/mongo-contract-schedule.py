"""Balance existing nextest partitions without changing their test selection."""

import math


def positive_integer(value):
    if type(value) is not int or value <= 0:
        raise ValueError("Scheduling costs and bounds must be positive integers")
    return value


def partition_costs(metadata, count, isolated, model):
    positive_integer(count)
    if not isinstance(model, dict) or set(model) != {"default_seconds", "fixture_seconds", "tests"}:
        raise ValueError("Invalid Mongo scheduling model")
    default = positive_integer(model["default_seconds"])
    costs = [positive_integer(model["fixture_seconds"])] * count
    timings = model["tests"]
    if not isinstance(timings, dict):
        raise TypeError("Invalid Mongo test timing inventory")
    for name, duration in timings.items():
        if not isinstance(name, str) or not name:
            raise ValueError("Invalid Mongo test timing name")
        positive_integer(duration)
    for suite in metadata["rust-suites"].values():
        names = sorted(name for name, test in suite["testcases"].items()
                       if test["filter-match"]["status"] == "matches" and name not in isolated)
        # Nextest's count partitioner is round-robin within each binary after
        # filtering. A full-partition default avoids fitting tiny size differences.
        baseline = math.ceil(len(names) / count) * default
        costs = [cost + baseline for cost in costs]
        for index, name in enumerate(names):
            costs[index % count] += timings.get(name, default) - default
    return costs


def assign_partitions(costs, shards, concurrency):
    """Start expensive fixtures first, using the existing per-runner slots."""
    positive_integer(shards)
    positive_integer(concurrency)
    if not isinstance(costs, list) or not costs:
        raise ValueError("A nonempty partition cost inventory is required")
    for cost in costs:
        positive_integer(cost)
    workers = [[0] * concurrency for _ in range(shards)]
    assigned = [[] for _ in range(shards)]
    for partition in sorted(range(len(costs)), key=lambda index: (-costs[index], index)):
        shard = min(range(shards), key=lambda index: (min(workers[index]), sum(workers[index]), index))
        worker = min(range(concurrency), key=lambda index: (workers[shard][index], index))
        workers[shard][worker] += costs[partition]
        assigned[shard].append(partition + 1)
    return assigned
