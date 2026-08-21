const term = document.querySelector("#terminal");

let pending = false;

const fit = () => {
  pending = false;

  const xterm = term.terminal;
  const screen = xterm?.element?.querySelector(".xterm-screen");
  if (!screen) return;

  const cellWidth = screen.clientWidth / xterm.cols;
  const cellHeight = screen.clientHeight / xterm.rows;
  if (!cellWidth || !cellHeight) return;

  const rect = xterm.element.getBoundingClientRect();
  const cols = Math.max(
    2,
    Math.floor((window.innerWidth - rect.left * 2) / cellWidth),
  );
  const rows = Math.max(
    1,
    Math.floor((window.innerHeight - rect.top * 2) / cellHeight),
  );

  if (cols !== xterm.cols || rows !== xterm.rows) xterm.resize(cols, rows);
};

requestAnimationFrame(fit)

window.addEventListener("resize", () => {
  if (pending) return;
  pending = true;
  requestAnimationFrame(fit);
});

document.addEventListener("py:ready", fit);
