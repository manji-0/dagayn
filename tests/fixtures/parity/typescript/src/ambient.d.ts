declare function declaredFn(a: number): string;
declare const VERSION: string;
declare let mutableGlobal: number;
declare class DeclaredClass {
  method(): void;
  prop: string;
}
declare abstract class DeclaredAbstract {
  abstract m(): void;
}
declare namespace NS {
  function nsFn(): void;
  interface NsOpts { x: number }
}
export interface Exported { e: 1 }
export type ExportedAlias = Exported;
export declare function exportedDeclared(): void;
export as namespace MyLib;
