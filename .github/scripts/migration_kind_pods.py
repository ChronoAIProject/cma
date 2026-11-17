"""Pod selection for image-bound migration fixture stages."""


class PodSelectionError(RuntimeError):
    """The deployment did not expose exactly one ready selected reader."""


def select_ready_api_pod(document, expected_alias, expected_config):
    """Return the sole ready pod matching alias and immutable image ID."""
    rows = []
    for pod in document.get("items", []):
        status = pod.get("status", {})
        containers = status.get("containerStatuses") or []
        if status.get("phase") != "Running" or len(containers) != 1:
            continue
        container = containers[0]
        if not container.get("ready") or container.get("restartCount", 0):
            continue
        image = container.get("image", "")
        image_id = container.get("imageID", "")
        if image not in (expected_alias, f"{expected_alias}:latest") and not image.endswith(
            f"/{expected_alias}:latest"
        ):
            continue
        if expected_config not in image_id:
            continue
        rows.append({
            "name": pod.get("metadata", {}).get("name"),
            "uid": pod.get("metadata", {}).get("uid"),
            "image": image,
            "image_id": image_id,
            "restart_count": container.get("restartCount", 0),
        })
    if len(rows) != 1:
        raise PodSelectionError(
            f"expected exactly one ready API pod for {expected_alias}, found {len(rows)}"
        )
    return rows[0]
