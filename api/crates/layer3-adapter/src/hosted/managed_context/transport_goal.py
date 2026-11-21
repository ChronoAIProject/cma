"""Read-only goal absence proof for an exact retained Codex conversation."""
import json
from contextlib import closing
from pathlib import Path
import sqlite3
import stat
import sys
import uuid

HOME_PREFIX = "/home/"


def inspect(home, session):
    if str(uuid.UUID(session)) != session:
        raise ValueError("session")
    root = Path(home)
    if not root.is_absolute() or not home.startswith(HOME_PREFIX) or root.resolve() != root:
        raise ValueError("home")

    def open_db(name, table, column):
        path = root / name
        if not stat.S_ISREG(path.lstat().st_mode) or path.resolve() != path:
            raise ValueError("database")
        db = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=0.2)
        try:
            db.execute("PRAGMA query_only=ON")
            db.execute("BEGIN")
            fields = db.execute("PRAGMA table_info(" + table + ")").fetchall()
            if not any(row[1] == column and row[2] == "TEXT" and row[5] == 1 for row in fields):
                raise ValueError("schema")
        except Exception:
            db.close()
            raise
        return db

    with closing(open_db("state_5.sqlite", "threads", "id")) as threads, closing(
        open_db("goals_1.sqlite", "thread_goals", "thread_id")
    ) as goals:
        exists = threads.execute("SELECT id FROM threads WHERE id=? LIMIT 1", (session,)).fetchone()
        goal = goals.execute("SELECT thread_id FROM thread_goals WHERE thread_id=? LIMIT 1", (session,)).fetchone()
        return exists == (session,) and goal is None


if __name__ == "__main__":
    result = {"version": 1, "goal_absent": False}
    try:
        request = json.loads(sys.stdin.read(4097))
        assert set(request) == {"home", "session"}
        result.update(session=request["session"], goal_absent=inspect(request["home"], request["session"]))
    except Exception:
        pass
    print(json.dumps(result))
