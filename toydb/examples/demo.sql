-- A tour of toydb's SQL. Run it with:
--   cargo run -- demo.db < examples/demo.sql
-- or inside the shell with:  .read examples/demo.sql

DROP TABLE IF EXISTS employees;
CREATE TABLE employees (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL,
    dept    TEXT,
    salary  INTEGER,
    remote  BOOLEAN
);

INSERT INTO employees VALUES
    (1, 'Ada',     'Engineering', 120000, TRUE),
    (2, 'Grace',   'Engineering', 135000, FALSE),
    (3, 'Alan',    'Research',    110000, TRUE),
    (4, 'Barbara', 'Research',    125000, FALSE),
    (5, 'Edsger',  'Engineering',  99000, TRUE),
    (6, 'Donald',  NULL,           90000, NULL);

-- Filtering, expressions and ordering
SELECT name, salary / 1000 || 'k' AS pay FROM employees
WHERE salary > 100000 AND name LIKE '%a%'
ORDER BY salary DESC;

-- Aggregates with GROUP BY / HAVING
SELECT dept, COUNT(*) AS people, AVG(salary) AS avg_salary, MAX(salary)
FROM employees
GROUP BY dept
HAVING COUNT(*) > 1
ORDER BY avg_salary DESC;

-- NULL handling: three-valued logic and COALESCE
SELECT name, coalesce(dept, '(none)') AS dept FROM employees WHERE remote IS NULL OR NOT remote;

-- Primary-key lookups use the B+tree directly
EXPLAIN SELECT name FROM employees WHERE id = 4;
EXPLAIN SELECT name FROM employees WHERE id BETWEEN 2 AND 4 ORDER BY name;

-- Transactions
BEGIN;
UPDATE employees SET salary = salary * 2;
SELECT SUM(salary) AS while_in_txn FROM employees;
ROLLBACK;
SELECT SUM(salary) AS after_rollback FROM employees;

-- The catalog is itself a table
SELECT name, rootpage FROM toydb_schema;
