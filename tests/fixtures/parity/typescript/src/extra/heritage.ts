import * as ns from "../lib/base";
import { Repo, Service } from "../interfaces";
export class G1 extends ns.Base implements Service<string>, ns.Marker {}
export class G2 extends Array<number> {}
export interface I2 extends Service<number>, ns.Marker {}
export default class extends ns.Base {}
export class WithThis {
  a() { this.b(); }
  b() {}
  c = () => { this.a(); };
  static s() { WithThis.s2(); }
  static s2() {}
}
export class Other { b() {} }
export function outer() {
  const inner = () => 1;
  function inner2() { return inner(); }
  return inner2();
}
export const wrapped = memoize(() => outer());
declare function memoize<T>(f: T): T;
