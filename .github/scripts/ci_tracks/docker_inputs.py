"""Resolve local inputs through Docker stages; reject ambiguous recipes."""

import fnmatch
from functools import lru_cache
import json
from pathlib import PurePosixPath
import re
import shlex


def instructions(text):
    # Heredoc bodies are shell input, never Docker instructions.
    pending, heredoc = '', None
    for line in text.splitlines():
        if heredoc:
            if line.strip() == heredoc:
                heredoc = None
            continue
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        pending += line.rstrip().removesuffix('\\') + ' '
        if line.rstrip().endswith('\\'):
            continue
        match = re.fullmatch(r'([A-Z]+)\s+(.*)', pending.strip(), re.S)
        if not match:
            raise ValueError('unsupported Docker instruction')
        op, value = match.groups()
        token = re.search(r'<<-?[\'\"]?(\w+)[\'\"]?', value)
        if token:
            heredoc = token[1]
        yield op, value
        pending = ''
    if pending or heredoc:
        raise ValueError('incomplete Docker instruction')


def expand(value, args):
    def replace(match):
        key = match[1] or match[2]
        if key not in args:
            raise ValueError('unresolved Docker input variable')
        return args[key]
    return re.sub(r'\$\{(\w+)\}|\$(\w+)', replace, value)


def local_source(value):
    value = value.removeprefix('./').rstrip('/')
    if not value or value.startswith('/') or '..' in PurePosixPath(value).parts or '://' in value:
        raise ValueError('unsupported Docker local source')
    return value


def stage_sources(text, target, build_args):
    stages, names, args = [], {}, dict(build_args)
    current = None
    for op, value in instructions(text):
        if op == 'ARG':
            key, sep, default = value.partition('=')
            if sep:
                args.setdefault(key, default)
        elif op == 'FROM':
            tokens = shlex.split(value)
            tokens = [token for token in tokens if not token.startswith('--')]
            base = expand(tokens[0], args)
            current = {'sources': set(), 'parents': set()}
            if base in names:
                current['parents'].add(names[base])
            elif base.isdigit() or ('$' in base):
                raise ValueError('unresolved Docker base stage')
            index = len(stages)
            stages.append(current)
            names[str(index)] = index
            if len(tokens) == 3 and tokens[1].lower() == 'as':
                if tokens[2] in names:
                    raise ValueError('duplicate Docker stage')
                names[tokens[2]] = index
        elif current is None:
            raise ValueError('Docker instruction before FROM')
        elif op in ('COPY', 'ADD'):
            flags = {}
            while value.startswith('--'):
                flag, _, value = value.partition(' ')
                key, sep, val = flag[2:].partition('=')
                if key == 'link' and (not sep or val in {'true', 'false'}):
                    continue
                if not sep or key not in {'from', 'chown', 'chmod'}:
                    raise ValueError('unsupported Docker copy flag')
                flags[key] = val
                value = value.lstrip()
            items = json.loads(value) if value.startswith('[') else shlex.split(value)
            if not isinstance(items, list) or len(items) < 2 or any(not isinstance(s, str) for s in items):
                raise ValueError('invalid Docker copy')
            if 'from' in flags:
                source = expand(flags['from'], args)
                if source not in names:
                    # Named build contexts are opaque unless excluded by the
                    # selected target (e.g. Gateway's local-preview target).
                    current['parents'].add('external:' + source)
                else:
                    current['parents'].add(names[source])
            else:
                current['sources'].update(local_source(expand(s, args)) for s in items[:-1])
        elif op == 'RUN':
            for mount in re.findall(r'--mount=([^\s]+)', value):
                options = dict((s.split('=', 1) if '=' in s else (s, '')) for s in mount.split(','))
                kind = options.get('type', 'bind')
                if kind in {'cache', 'secret', 'ssh', 'tmpfs'}:
                    continue
                if kind != 'bind':
                    raise ValueError('unsupported Docker mount')
                source_stage = options.get('from')
                if source_stage:
                    if source_stage not in names:
                        raise ValueError('unresolved build mount stage')
                    current['parents'].add(names[source_stage])
                else:
                    source = options.get('source', options.get('src', '.'))
                    current['sources'].add(local_source(expand(source, args)))
        elif op not in {'ENV', 'WORKDIR', 'USER', 'LABEL', 'EXPOSE', 'ENTRYPOINT', 'CMD', 'SHELL', 'HEALTHCHECK', 'STOPSIGNAL', 'VOLUME'}:
            raise ValueError('unsupported Docker instruction')
    index = names[target] if target else len(stages) - 1
    pending, seen, sources = [index], set(), set()
    while pending:
        index = pending.pop()
        if isinstance(index, str):
            raise ValueError('selected image uses an unresolved external build context')
        if index in seen:
            continue
        seen.add(index)
        sources.update(stages[index]['sources'])
        pending.extend(stages[index]['parents'])
    return sources


def pattern_regex(pattern):
    output, index = '', 0
    while index < len(pattern):
        char = pattern[index]
        if char == '*':
            if pattern[index:index + 2] == '**':
                index += 1
                if pattern[index + 1:index + 2] == '/':
                    output += '(?:.*/)?'
                    index += 1
                else:
                    output += '.*'
            else:
                output += '[^/]*'
        elif char == '?':
            output += '[^/]'
        elif char == '[':
            end = pattern.find(']', index + 1)
            if end < 0:
                raise ValueError('invalid Docker input pattern')
            output += pattern[index:end + 1]
            index = end
        else:
            output += re.escape(char)
        index += 1
    return output


@lru_cache(maxsize=1024)
def compiled_pattern(pattern):
    # A comparison applies the same small rule set to thousands of paths.
    # Cache only pure string calculations, never filesystem observations.
    return re.compile('^' + pattern_regex(pattern) + '$')


@lru_cache(maxsize=16384)
def path_parts(path):
    parts = path.split('/')
    return tuple(parts), tuple('/'.join(parts[:n]) for n in range(1, len(parts) + 1))


def matches(path, pattern):
    pattern = pattern.strip('/')
    if pattern == '.':
        return True
    if '*' not in pattern and '?' not in pattern and '[' not in pattern:
        return path == pattern or path.startswith(pattern + '/')
    regex = compiled_pattern(pattern)
    # A matching directory includes its descendants; '*' itself cannot cross
    # a slash, while '**' follows Docker's recursive directory semantics.
    return any(regex.fullmatch(prefix) for prefix in path_parts(path)[1])


@lru_cache(maxsize=128)
def ignore_patterns(text):
    patterns = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith('#') or line == '.':
            continue
        negate = line.startswith('!')
        pattern = line[1:] if negate else line
        patterns.append((negate, pattern))
    return tuple(patterns)


# Immutable snapshots often contain the same paths and Dockerignore rules.
# Apply the complete ordered rule set once for each exact pair.
@lru_cache(maxsize=32768)
def ignored(path, text):
    result = False
    parts = path_parts(path)[0]
    for negate, pattern in ignore_patterns(text):
        if matches(path, pattern) or ('/' not in pattern and any(fnmatch.fnmatchcase(part, pattern) for part in parts)):
            result = not negate
    return result
