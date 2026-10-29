# User authentication schema

Design the SQLite schema for a user authentication system and write it to `schema.sql`. The service never stores a plaintext
password and never stores a raw session or reset token; it stores only hashes of them.

1. `users`: `id` (integer primary key); `email` (required, unique, case-insensitive: `A@x.io` and `a@x.io` are the same
   address); `password_hash` (required); `display_name` (required); `is_active` (required, default 1); `created_at`
   (required, default the current timestamp). There is no `password` column.
2. `roles`: `id` (integer primary key); `name` (required, unique). Roles are rows, not code.
3. `user_roles`: links users to roles. A user holds a role at most once. Deleting a user or a role removes its links.
4. `sessions`: `id` (integer primary key); `user_id` (required, a foreign key to `users`; deleting the user deletes its
   sessions); `token_hash` (required, unique); `created_at` (required, default the current timestamp); `expires_at`
   (required). Sessions are looked up by `user_id`, so `user_id` must be indexed.
5. `password_resets`: `id` (integer primary key); `user_id` (required, a foreign key to `users`, deleting the user deletes
   its resets); `token_hash` (required, unique); `expires_at` (required); `used_at` (nullable: null until the reset is used).

Use SQLite types. The checker turns foreign keys on itself, so the schema does not need to.
