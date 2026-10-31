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
    return {row[1].lower(): row for row in conn.execute(f"PRAGMA table_info({table})")}


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

user_cols = columns("users")
for name in ["id", "email", "password_hash", "display_name", "is_active", "created_at"]:
    if name not in user_cols:
        failures.append(f"users.{name} is missing")
if "password" in user_cols:
    failures.append("users has a plaintext password column")

# users: required fields, defaults, case-insensitive unique email
must_work("insert a user", "INSERT INTO users (email, password_hash, display_name) VALUES (?, ?, ?)",
          ("Ann@Example.com", "hash-1", "Ann"))
row = conn.execute("SELECT is_active, created_at FROM users WHERE email = ?", ("Ann@Example.com",)).fetchone()
if row is None or row[0] != 1 or row[1] is None:
    failures.append("users: is_active should default to 1 and created_at should be set")
must_fail("duplicate email, same case", "INSERT INTO users (email, password_hash, display_name) VALUES (?, ?, ?)",
          ("Ann@Example.com", "hash-2", "Ann again"))
must_fail("duplicate email, other case", "INSERT INTO users (email, password_hash, display_name) VALUES (?, ?, ?)",
          ("ann@example.COM", "hash-3", "Ann lower"))
must_fail("missing password_hash", "INSERT INTO users (email, display_name) VALUES (?, ?)",
          ("bob@example.com", "Bob"))
must_fail("missing display_name", "INSERT INTO users (email, password_hash) VALUES (?, ?)",
          ("cy@example.com", "hash-4"))
must_work("second user", "INSERT INTO users (email, password_hash, display_name) VALUES (?, ?, ?)",
          ("bob@example.com", "hash-5", "Bob"))

# roles: unique names
must_work("insert a role", "INSERT INTO roles (name) VALUES (?)", ("admin",))
must_work("insert a second role", "INSERT INTO roles (name) VALUES (?)", ("reader",))
must_fail("duplicate role name", "INSERT INTO roles (name) VALUES (?)", ("admin",))

# user_roles: a role at most once, cascades from both sides
def id_of(sql, params, label):
    row = conn.execute(sql, params).fetchone()
    if row is None:
        failures.append(f"{label} is missing, so the later checks cannot run")
        print("\n".join(failures))
        sys.exit(1)
    return row[0]


ann = id_of("SELECT id FROM users WHERE email = ?", ("Ann@Example.com",), "the first user")
bob = id_of("SELECT id FROM users WHERE email = ?", ("bob@example.com",), "the second user")
admin = id_of("SELECT id FROM roles WHERE name = ?", ("admin",), "the admin role")
reader = id_of("SELECT id FROM roles WHERE name = ?", ("reader",), "the reader role")
must_work("grant admin to ann", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, admin))
must_work("grant reader to ann", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, reader))
must_work("grant reader to bob", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (bob, reader))
must_fail("the same role twice", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, admin))
must_fail("a link to a missing role", "INSERT INTO user_roles (user_id, role_id) VALUES (?, ?)", (ann, 999))

# sessions: a session belongs to a user, its token is unique, user_id is indexed
must_work("open a session for ann", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (ann, "sha-a", "2030-01-01T00:00:00Z"))
must_work("open a session for bob", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (bob, "sha-b", "2030-01-01T00:00:00Z"))
must_fail("duplicate token_hash", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (bob, "sha-a", "2030-01-01T00:00:00Z"))
must_fail("session for a missing user", "INSERT INTO sessions (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (999, "sha-c", "2030-01-01T00:00:00Z"))
must_fail("session without expires_at", "INSERT INTO sessions (user_id, token_hash) VALUES (?, ?)",
          (ann, "sha-d"))
indexed = False
for index in conn.execute("PRAGMA index_list(sessions)").fetchall():
    first = conn.execute(f"PRAGMA index_info('{index[1]}')").fetchone()
    if first and first[2] == "user_id":
        indexed = True
if not indexed:
    failures.append("sessions.user_id is not indexed")

# password_resets: unique token hash, optional used_at
must_work("request a reset", "INSERT INTO password_resets (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (ann, "reset-1", "2030-01-01T00:00:00Z"))
must_fail("duplicate reset token", "INSERT INTO password_resets (user_id, token_hash, expires_at) VALUES (?, ?, ?)",
          (bob, "reset-1", "2030-01-01T00:00:00Z"))

# cascades: deleting a user removes its links, sessions and resets; deleting a role removes its links
conn.execute("DELETE FROM users WHERE id = ?", (ann,))
for table in ["user_roles", "sessions", "password_resets"]:
    left = conn.execute(f"SELECT COUNT(*) FROM {table} WHERE user_id = ?", (ann,)).fetchone()[0]
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
