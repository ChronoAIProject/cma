"""Preserve the managed PATH through Debian's exact login profile."""
from pathlib import Path

def rewrite(path):
    profile = path.read_text()
    defaults = '\n'.join([
        'if [ "$(id -u)" -eq 0 ]; then',
        '  PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"',
        'else',
        '  PATH="/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games"',
        'fi', 'export PATH',
    ])
    if profile.count(defaults) != 1:
        raise SystemExit('unexpected Debian login PATH defaults')
    fallback = '\n'.join(['if [ -z "${PATH:-}" ]; then',
        *['  ' + line.replace('$(id -u)', '$(/usr/bin/id -u)')
          for line in defaults.splitlines()[:-1]], 'fi', 'export PATH'])
    path.write_text(profile.replace(defaults, fallback))


if __name__ == '__main__':
    rewrite(Path('/etc/profile'))
