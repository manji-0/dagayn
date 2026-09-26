import { util } from "@lib/base";

export function decl(a: number): number {
  function nested(): number { return util(a); }
  return nested();
}

export function over(a: string): string;
export function over(a: number): number;
export function over(a: any): any { return a; }

export const arrow = (x: number): number => x + 1;
export let fnExpr = function (y: number) { return y; };
const namedExpr = function inner() { return 1; };
export async function asyncFn(): Promise<void> {}
export function* gen() { yield 1; }
export async function* agen() { yield 2; }

(function iife() { decl(1); })();
(() => { arrow(1); })();

export const api = {
  get(id: string) { return decl(1); },
  post: (x: number) => arrow(x),
  put: function () { return 1; },
  nested: { deep() { return 2; } },
};

export default function () {
  return decl(2);
}

export function withCallbacks(items: number[]) {
  items.map((x) => arrow(x));
  setTimeout(function later() { decl(3); }, 10);
  return items.filter(Boolean);
}

let reassigned;
reassigned = () => 1;
const { a, b } = { a: () => 1, b: 2 };
