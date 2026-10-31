import sqlite3
import unittest
from pathlib import Path


class SchemaTest(unittest.TestCase):
    def setUp(self):
        self.conn = sqlite3.connect(":memory:")
        self.conn.execute("PRAGMA foreign_keys = ON")
        self.conn.executescript(Path("schema.sql").read_text(encoding="utf-8"))

    def columns(self, table):
        return [row[1].lower() for row in self.conn.execute(f"PRAGMA table_info({table})")]

    def test_users_has_at_most_five_columns_including_id(self):
        self.assertLessEqual(len(self.columns("users")), 5)

    def test_users_holds_every_login_field(self):
        for name in ["id", "email", "password_hash", "display_name", "is_active", "created_at"]:
            self.assertIn(name, self.columns("users"))

    def test_email_is_unique_regardless_of_case(self):
        self.conn.execute("INSERT INTO users (email, password_hash, display_name) VALUES (?, ?, ?)",
                          ("A@x.io", "hash", "A"))
        with self.assertRaises(sqlite3.IntegrityError):
            self.conn.execute("INSERT INTO users (email, password_hash, display_name) VALUES (?, ?, ?)",
                              ("a@X.IO", "hash", "A again"))


if __name__ == "__main__":
    unittest.main()
