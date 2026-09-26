export namespace Outer {
  export const x = 1;
  export function helper(): number { return x; }
  export class Inner {
    run() { helper(); }
  }
  export namespace Deep {
    export function deepFn() {}
  }
}

namespace A.B.C {
  export function abc() {}
}

module Legacy {
  export function old() {}
}

declare module "external-lib" {
  export function ext(): void;
  export interface ExtOptions { a: number }
}

declare global {
  interface Window { myGlobal: string }
  function globalFn(): void;
}

export function useNs() {
  Outer.helper();
  new Outer.Inner().run();
  A.B.C.abc();
  Outer.Deep.deepFn();
}
