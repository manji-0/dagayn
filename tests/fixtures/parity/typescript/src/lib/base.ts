export class Base {
  constructor(public dep: unknown) {}
  toString(): string { return "base"; }
}
type Ctor<T = {}> = new (...args: any[]) => T;
export function Mixin<TBase extends Ctor>(B: TBase) {
  return class extends B {
    mixed() {}
  };
}
export const util = (x: number) => x * 2;
