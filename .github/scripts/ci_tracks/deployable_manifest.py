"""Current publication identities and the bounded historical manifest format."""

CURRENT = {
    "cma-agent-manager": "api",
    "cma-event-gateway": "cmaeg",
    "cma-web": "web",
    "cma-runtime": "runtime",
    "cma-runtime-omarchy": "runtime-omarchy",
}
RETIRED = {
    "cma-image-manager": "image-manager",
    "cma-runtime-desktop": "runtime-desktop",
}


def validate_names(data, *, historical=False):
    allowed = (set(CURRENT), set(CURRENT) | set(RETIRED)) if historical else (set(CURRENT),)
    if not isinstance(data, dict) or set(data) not in allowed:
        raise ValueError("incomplete image manifest")
    known = CURRENT | RETIRED
    if any(not isinstance(recipe, dict) or recipe.get("component") != known[name]
           for name, recipe in data.items()):
        raise ValueError("ambiguous image components")
    return data
