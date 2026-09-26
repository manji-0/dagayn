import { decl, arrow } from "../functions";
import { Box } from "../classes";

describe("functions", () => {
  beforeEach(() => { arrow(0); });
  it("decl works", () => {
    expect(decl(1)).toBe(2);
  });
  test.each([1, 2])("arrow %i", (n) => {
    arrow(n);
  });
  it.only("only", async () => { await Promise.resolve(decl(3)); });
  describe.skip("nested", () => {
    test("box", () => { new Box({} as never).helper(); });
  });
});

function testHelper() { decl(9); }
