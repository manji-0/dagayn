export interface Repo {
  find(id: string): string;
  readonly name?: string;
}

export interface Logger {
  log(msg: string): void;
}

export interface Service<T> extends Repo, Logger {
  run(input: T): Promise<T>;
  (call: number): void;
  new (x: number): Service<T>;
  [key: string]: unknown;
}

// declaration merging
export interface Repo {
  save(item: string): void;
}
