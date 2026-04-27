def build_graph(repo_path: str) -> dict:
    return {"repo": repo_path, "nodes": [], "edges": []}


def run_analysis(graph: dict) -> None:
    print(f"Analysing {graph['repo']}")
