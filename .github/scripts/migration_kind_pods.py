"""Pod selection for image-bound migration fixture stages."""


class PodSelectionError(RuntimeError):
    """The deployment did not expose exactly one ready selected reader."""


def matches_image(image, expected):
    if '/' in expected or ':' in expected or '@' in expected:
        return image == expected
    return image in (expected, expected + ':latest', 'docker.io/library/' + expected,
                     'docker.io/library/' + expected + ':latest')


def matches_identity(image_id, image, config):
    for prefix in ('containerd://', 'docker-pullable://'):
        if image_id.startswith(prefix):
            image_id = image_id[len(prefix):]
            break
    accepted = {config}
    if '@sha256:' in image:
        accepted.update((image, image.split('@')[1]))
    return image_id in accepted


def select_ready_api_pod(document, expected_alias, expected_config):
    """Bind the declared reader alias and immutable running image identity."""
    rows = []
    for pod in document.get("items", []):
        status = pod.get("status", {})
        containers = status.get("containerStatuses") or []
        spec = pod.get("spec", {}).get("containers", [])
        if status.get("phase") != "Running" or len(containers) != 1 or len(spec) != 1:
            continue
        container = containers[0]
        if (spec[0].get("name") != "api" or container.get("name") != "api"
                or not container.get("ready") or container.get("restartCount", 0)):
            continue
        image = container.get("image", "")
        image_id = container.get("imageID", "")
        if not matches_image(spec[0].get("image", ""), expected_alias):
            continue
        if not matches_identity(image_id, expected_alias, expected_config):
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
