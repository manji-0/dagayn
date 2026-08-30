# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "marimo",
# ]
# ///

import marimo

__generated_with = "0.13.0"
app = marimo.App(width="medium")

with app.setup:
    import os
    from pathlib import Path

    CONSTANT = 1


@app.cell
def _():
    import marimo as mo

    return (mo,)


@app.cell(hide_code=True)
def _(mo):
    mo.md(
        r"""
        # Analysis
        """
    )
    return


@app.function
def add(x, y):
    return x + y


@app.function
def multiply(x, y):
    return x * y


@app.class_definition
class DataProcessor:
    def __init__(self, factor):
        self.factor = factor

    def process(self, value):
        return add(value, self.factor)


@app.cell
def _(mo, add, multiply, DataProcessor):
    def helper(n):
        return multiply(n, 2)

    result = add(1, 2)
    processed = DataProcessor(3).process(result)
    _df = mo.sql(
        f"""
        SELECT * FROM bronze.events
        JOIN silver.users ON events.user_id = users.id
        """
    )
    return helper, result, processed, _df


if __name__ == "__main__":
    app.run()
