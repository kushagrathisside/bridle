import sqlite3
import sys
from pathlib import Path

schema = Path("schema.sql")
if not schema.is_file():
    print("no schema.sql")
    sys.exit(1)
conn = sqlite3.connect(":memory:")
conn.execute("PRAGMA foreign_keys = ON")
try:
    conn.executescript(schema.read_text(encoding="utf-8"))
except sqlite3.Error as error:
    print("schema.sql does not load:", error)
    sys.exit(1)

failures = []


def columns(table):
    return [row[1].lower() for row in conn.execute(f"PRAGMA table_info({table})")]


def must_fail(label, sql, params=()):
    try:
        conn.execute(sql, params)
    except sqlite3.IntegrityError:
        return
    except sqlite3.Error as error:
        failures.append(f"{label}: {error}")
        return
    failures.append(f"{label}: the write was accepted")


def must_work(label, sql, params=()):
    try:
        conn.execute(sql, params)
    except sqlite3.Error as error:
        failures.append(f"{label}: {error}")


for table in ["users", "roles", "user_roles", "sessions", "password_resets"]:
    if not columns(table):
        failures.append(f"table {table} is missing")
if failures:
    print("\n".join(failures))
    sys.exit(1)

if len(columns("users")) > 5:
    failures.append(f"users has {len(columns('users'))} columns; at most 5 are allowed")
for name in ["email", "password_hash", "is_active", "created_at"]:
    if name not in columns("users"):
        failures.append(f"users.{name} is missing")
if "password" in columns("users") or "display_name" in columns("users"):
    failures.append("users holds a plaintext password or a display name")
profile = None
for (table,) in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'").fetchall():
    if table != "users" and "display_name" in columns(table):
        profile = table
if profile is None:
    failures.append("no table holds the display name")
    print("\n".join(failures))
    sys.exit(1)
profile_key = [row[3] for row in conn.execute(f"PRAGMA foreign_key_list({profile})") if row[2] == "users"]
if not profile_key:
    failures.append(f"{profile} has no foreign key to users")
    print("\n".join(failures))
    sys.exit(1)
key = profile_key[0]

must_work("insert a user", "INSERT INTO users (email, password_hash) VALUES (?, ?)", ("Ann@Example.com", "h1"))
row = conn.execute("SELECT is_active, created_at FROM users WHERE email = ?", ("Ann@Example.com",)).fetchone()
if row is None or row[0] != 1 or row[1] is None:
    failures.append("users: is_active should default to 1 and created_at should be set")
must_fail("duplicate email, other case", "INSERT INTO users (email, password_hash) VALUES (?, ?)",
          ("ann@example.COM", "h2"))
must_fail("missing password_hash", "INSERT INTO users (email) VALUES (?)", ("bob@example.com",))
must_work("second user", "INSERT INTO users (email, password_hash) VALUES (?, ?)", ("bob@example.com", "h3"))
ann_row = conn.execute("SELECT id FROM users WHERE email = ?", ("Ann@Example.com",)).fetchone()
bob_row = conn.execute("SELECT id FROM users WHERE email = ?", ("bob@example.com",)).fetchone()
if ann_row is None or bob_row is None:
    failures.append("the users cannot be read back")
    print("\n".join(failures))
    sys.exit(1)
ann, bob = ann_row[0], bob_row[0]

must_work("display name for ann", f"INSERT INTO {profile} ({key}, display_name) VALUES (?, ?)", (ann, "Ann"))
must_fail("second display name for ann", f"INSERT INTO {profile} ({key}, display_name) VALUES (?, ?)",
          (ann, "Ann 2"))
must_fail("display name without a value", f"INSERT INTO {profile} ({key}) VALUES (?)", (bob,))
must_fail("display name for a missing user", f"INSERT INTO {profile} ({key}, display_name) VALUES (?, ?)",
          (999, "Ghost"))

must_work("insert a role", "INSERT INTO roles (name) VALUES (?)", ("admin",))
must_work("insert a second role", "INSERT INTO roles (name) VALUES (?)", ("reader",))
must_fail("duplicate role name", "INSERT INTO roles (name) VALUES (?)", ("admin",))
admin = conn.execute("SELECT id FROM roles WHERE name = 'admin'").fetchone()[0]
reader = conn.execute("SELECT id FROM roles WHERE name = 'reader'").fetchone()[0]
must_work("grant admin to ann", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, admin))
must_work("grant reader to ann", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, reader))
must_fail("the same role twice", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, admin))

must_work("session for ann", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (ann, "sha-a", "2030-01-01T00:00:00Z"))
must_fail("duplicate token_hash", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (bob, "sha-a", "2030-01-01T00:00:00Z"))
must_fail("session for a missing user", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (999, "sha-z", "2030-01-01T00:00:00Z"))
indexed = any(
    (info := conn.execute(f"PRAGMA index_info('{idx[1]}')").fetchone()) and info[2] == "user_id"
    for idx in conn.execute("PRAGMA index_list(sessions)").fetchall()
)
if not indexed:
    failures.append("sessions.user_id is not indexed")

must_work("reset for ann", "INSERT INTO password_resets (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (ann, "reset-1", "2030-01-01T00:00:00Z"))
must_fail("duplicate reset token", "INSERT INTO password_resets (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (bob, "reset-1", "2030-01-01T00:00:00Z"))

conn.execute("DELETE FROM users WHERE id = ?", (ann,))
for table in [profile, "user_roles", "sessions", "password_resets"]:
    left = conn.execute(f"SELECT COUNT(*) FROM {table} WHERE {key if table == profile else 'user_id'} = ?",
                        (ann,)).fetchone()[0]
    if left:
        failures.append(f"deleting a user left {left} row(s) in {table}")
conn.execute("DELETE FROM roles WHERE id = ?", (reader,))
if conn.execute("SELECT COUNT(*) FROM user_roles WHERE role_id = ?", (reader,)).fetchone()[0]:
    failures.append("deleting a role left user_roles rows")

if failures:
    print(f"{len(failures)} check(s) failed:")
    print("\n".join(f"  {line}" for line in failures))
    sys.exit(1)
print("schema checks passed")
sys.exit(0)
