# Authentication schema with a column cap

Write the SQLite schema of a user authentication system to `schema.sql`. The service stores only hashes of passwords and
tokens. Use SQLite types. Foreign keys are enforced.

1. `users`: `id` (integer primary key); `email` (required, unique, case-insensitive); `password_hash` (required);
   `is_active` (required, default 1); `created_at` (required, default the current timestamp). The `users` table has at
   most five columns, `id` included.
2. Every user has a display name (required, one per user). The schema stores it.
3. `roles`: `id` (integer primary key); `name` (required, unique).
4. `user_roles`: links users to roles. A user holds a role at most once. Deleting a user or a role removes its links.
5. `sessions`: `id` (integer primary key); `user_id` (required, a foreign key to `users`, deleting the user deletes its
   sessions); `token_hash` (required, unique); `expires_at` (required). `user_id` is indexed.
6. `password_resets`: `id` (integer primary key); `user_id` (required, a foreign key to `users`, cascading delete);
   `token_hash` (required, unique); `expires_at` (required); `used_at` (nullable).
