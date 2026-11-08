import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

// The real CodeMirror editor (other tests use the stand-in from src/test/setup.ts).
vi.unmock("./CodeEditor");
const { default: CodeEditor } = await import("./CodeEditor");

const samples: Record<string, string> = {
  ini: '[PHP]\n; comment\nmemory_limit = 128M\ndate.timezone = "UTC"\n',
  nginx: 'gzip on; # comment\nlocation / {\n  try_files $uri "/index.php";\n}\n',
  apache: '<Directory "C:/x">\n  Require all granted\n</Directory>\n',
  yaml: "net:\n  port: 27017\n  bindIp: '127.0.0.1' # loopback\n",
  text: "plain text\n",
};

describe("CodeEditor", () => {
  it.each(Object.entries(samples))("renders and highlights %s files", (language, sample) => {
    const onChange = vi.fn();
    const { container, unmount } = render(
      <CodeEditor
        value={sample}
        onChange={onChange}
        language={language}
        readOnly={false}
        label="config"
        markedLine={2}
      />,
    );
    const content = screen.getByRole("textbox", { name: "config" });
    expect(content).toHaveAttribute("contenteditable", "true");
    // Lines aren't separated by newlines in the DOM text; compare the first line.
    expect(content.textContent).toContain(sample.slice(0, sample.indexOf("\n")));
    expect(container.querySelector(".cm-marked-line")).not.toBeNull();
    expect(container.querySelector(".cm-lineNumbers")).not.toBeNull();
    unmount();
  });

  it("pushes outside changes in and stays read-only when asked", async () => {
    const { rerender } = render(
      <CodeEditor
        value="a = 1\n"
        onChange={() => undefined}
        language="ini"
        readOnly
        label="ro"
        markedLine={null}
      />,
    );
    const content = screen.getByRole("textbox", { name: "ro" });
    expect(content).toHaveAttribute("contenteditable", "false");
    rerender(
      <CodeEditor
        value="b = 2\n"
        onChange={() => undefined}
        language="ini"
        readOnly
        label="ro"
        markedLine={null}
      />,
    );
    await waitFor(() => expect(content.textContent).toContain("b = 2"));
  });
});
