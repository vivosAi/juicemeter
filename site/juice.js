// The page is a meter: the box in the corner drains as you scroll down.
(() => {
  const meter = document.querySelector(".page-meter");
  if (!meter) return;
  const box = meter.querySelector(".jb");
  const readout = meter.querySelector(".page-readout");

  const update = () => {
    const max = document.documentElement.scrollHeight - innerHeight;
    const read = max > 0 ? Math.min(1, scrollY / max) : 0;
    const left = Math.round(100 - read * 100);
    box.style.setProperty("--lvl", `${Math.max(left, 4)}%`);
    const dry = left <= 8;
    meter.classList.toggle("dry", dry);
    readout.textContent = dry ? "squeezed dry" : `${left}% left`;
  };

  addEventListener("scroll", update, { passive: true });
  addEventListener("resize", update);
  update();
})();
