"""Fixed publication stage identifiers are the entire public diagnostic surface."""
from enum import Enum


class PublicationStage(Enum):
    INPUT = "input"
    RESUME = "resume-verification"
    SNAPSHOT = "source-snapshot"
    IDENTITY = "build-identity"
    REUSE = "artifact-reuse"
    BUILD = "build"
    ABSENCE = "registry-absence"
    INSPECT = "archive-inspection"
    SCAN = "secret-scan"
    LOGIN = "registry-login"
    PUSH = "registry-push"
    VERIFY = "registry-verification"
    RECEIPT = "receipt"


class PublicationProgress:
    def __init__(self):
        self.current = PublicationStage.INPUT

    def enter(self, stage):
        if type(stage) is not PublicationStage:
            raise ValueError("Unknown publication stage")
        self.current = stage


def public_stage(progress):
    if type(progress) is PublicationProgress and type(progress.current) is PublicationStage:
        return progress.current.value
    return "unknown"
