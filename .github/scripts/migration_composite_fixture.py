#!/usr/bin/env python3
"""Compose independent HTTP compatibility extensions into one receipt."""
from migration_profile_fixture import main
from migration_trigger_fixture import TriggerFixture
from migration_usage_fixture import UsageFixture


class CompositeFixture:
    def __init__(self, fixture):
        self.trigger = TriggerFixture(fixture)
        self.usage = UsageFixture(fixture)
        self.output = fixture.output

    def exercise(self, stage, profiles):
        trigger = self.trigger.exercise(stage, profiles)
        usage = self.usage.exercise(stage, profiles)
        return {
            'collections': sorted(set(trigger['collections'] + usage['collections'])),
            'shapes': trigger['shapes'] + usage['shapes'],
            'samples': trigger['samples'] + usage['samples'],
            'checks': {**trigger['checks'], **usage['checks']},
        }


if __name__ == '__main__':
    main(CompositeFixture)
