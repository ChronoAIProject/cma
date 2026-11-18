"""A reviewed image path cannot be redirected through an archive link."""


class VectorPaths:
    def __init__(self, paths):
        self.parents = {
            "/".join(path.split("/")[:index]) for path in paths for index in range(1, len(path.split("/")))
        }
        self.redirected = False

    def observe(self, name, entry):
        # Do not resolve image-controlled ancestors. An earlier redirected
        # package location remains ineligible even if a later layer replaces it.
        if name in self.parents and not entry.isdir():
            self.redirected = True
