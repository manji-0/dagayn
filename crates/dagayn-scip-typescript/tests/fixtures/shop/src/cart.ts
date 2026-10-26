export function total(prices: number[]): number {
  return prices.reduce((sum, price) => sum + price, 0);
}

export class Cart {
  private items: number[] = [];

  add(price: number): void {
    this.items.push(price);
  }

  sum(): number {
    return total(this.items);
  }
}

export function format(value: number): string;
export function format(value: string): string;
export function format(value: number | string): string {
  return String(value);
}

export function discount(price: number): number {
  return Math.max(0, price - discount.flat);
}

export namespace discount {
  export const flat = 1;
}

export function describe(prices: number[]): string {
  function label(count: number): string {
    return `${count} items`;
  }
  return label(prices.length);
}
