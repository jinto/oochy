CREATE TABLE IF NOT EXISTS skill_schedule (
    skill_name TEXT PRIMARY KEY,
    last_run_at TEXT,
    failure_count INTEGER NOT NULL DEFAULT 0
);
