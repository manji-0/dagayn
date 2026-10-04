export class Panel {
  toJSON(): object {
    return {};
  }

  tsUnusedHelper(): number {
    return 1;
  }
}

export interface Shape {
  area(): number;
}
