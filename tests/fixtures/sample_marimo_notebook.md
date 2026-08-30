---
title: Sample marimo markdown notebook
marimo-version: 0.13.0
---

# Sample analysis

Prose around the executable cells.

```python {.marimo}
def add(x, y):
    return x + y
```

```python {.marimo name="scale"}
def multiply(x, y):
    return x * y
```

```sql {.marimo query="events"}
SELECT * FROM bronze.events
JOIN silver.users ON events.user_id = users.id
```

```python
def ignored():
    return 1
```
