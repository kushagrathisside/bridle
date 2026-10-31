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

users = [row[1].lower() for row in conn.execute("PRAGMA table_info(users)")]
login_fields = ["id", "email", "password_hash", "display_name", "is_active", "created_at"]
failures = []
if len(users) > 5:
    failures.append(f"users has {len(users)} columns; at most 5 are allowed")
for name in login_fields:
    if name not in users:
        failures.append(f"users.{name} is missing")
if failures:
    print("\n".join(failures))
    print("the README asks for all six login fields in users and at most five columns, which cannot both hold")
    sys.exit(1)
print("schema checks passed")
sys.exit(0)
