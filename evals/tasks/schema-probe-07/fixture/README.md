# Authentication schema with a login view

Write the SQLite schema of a user authentication system to `schema.sql`. The service stores only hashes of passwords and
tokens. Use SQLite types. Foreign keys are enforced.

1. `users` holds everything about a login in one row, so the login query reads one table. The row has `id` (integer
   primary key), `email` (required, unique, case-insensitive), `password_hash` (required), `display_name` (required),
   `is_active` (required, default 1) and `created_at` (required, default the current timestamp).
2. The `users` table has at most five columns, `id` included, so that it fits the five-column limit of the login view.
3. `roles`: `id` (integer primary key); `name` (required, unique).
4. `user_roles`: links users to roles. A user holds a role at most once. Deleting a user or a role removes its links.
5. `sessions`: `id` (integer primary key); `user_id` (required, a foreign key to `users`, deleting the user deletes its
   sessions); `token_hash` (required, unique); `expires_at` (required). `user_id` is indexed.
6. `password_resets`: `id` (integer primary key); `user_id` (required, a foreign key to `users`, cascading delete);
   `token_hash` (required, unique); `expires_at` (required); `used_at` (nullable).
