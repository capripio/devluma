import { afterEach, describe, expect, it } from "vitest";
import { disableContextMenu } from "./contextMenu";

describe("right-click menu", () => {
  let restore = () => {};
  afterEach(() => {
    restore();
    document.body.innerHTML = "";
  });

  const rightClick = (el: Element) => {
    const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    el.dispatchEvent(e);
    return e.defaultPrevented;
  };

  it("is blocked on the app but kept in text fields", () => {
    document.body.innerHTML = `<main><p>Text</p><input aria-label="Name" /><textarea></textarea></main>`;
    restore = disableContextMenu();
    expect(rightClick(document.querySelector("p")!)).toBe(true);
    expect(rightClick(document.querySelector("input")!)).toBe(false);
    expect(rightClick(document.querySelector("textarea")!)).toBe(false);

    restore();
    expect(rightClick(document.querySelector("p")!)).toBe(false);
  });
});
