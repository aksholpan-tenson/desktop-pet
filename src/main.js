const { getCurrentWindow, currentMonitor, PhysicalPosition } = window.__TAURI__.window;
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const appWindow = getCurrentWindow();
const pet = document.getElementById("pet");
const bubble = document.getElementById("bubble");
const item = document.getElementById("item");

// ================= НАСТРОЙКИ =================
const DRY_RUN = false; // true = только показывать, файлы НЕ трогать
const SPEED = 3;

const faces = { idle: "🐱", walk: "🐈", sleep: "😴", happy: "😸", work: "😼" };
const ICONS = {
  Documents: "📄", Images: "🖼️", Archives: "🗜️",
  Installers: "💿", Video: "🎬", Music: "🎵",
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ================= СОСТОЯНИЕ =================
let state = "idle";
let lastActive = Date.now();
let busy = false; // true, пока питомец занят (радуется, работает, отменяет)

function setState(name) {
  state = name;
  pet.className = name;
  pet.textContent = faces[name];
  if (name === "walk") {
    dir = Math.random() < 0.5 ? 1 : -1;
    syncPosition();
  }
}

function say(text) {
  bubble.textContent = text;
  bubble.style.display = text ? "block" : "none";
}

function shorten(name) {
  return name.length > 14 ? name.slice(0, 12) + "…" : name;
}

// ================= ХОДЬБА (этап 3) =================
let x = 0, y = 0, dir = 1;
let bounds = null;
let moving = false;

async function initBounds() {
  const mon = await currentMonitor();
  const area = mon.workArea ?? { position: mon.position, size: mon.size };
  const win = await appWindow.outerSize();
  bounds = {
    left: area.position.x,
    right: area.position.x + area.size.width - win.width,
    bottom: area.position.y + area.size.height - win.height,
  };
}

async function syncPosition() {
  const p = await appWindow.outerPosition();
  x = p.x;
  y = p.y;
}

function updateFlip() {
  pet.style.scale = dir === 1 ? "-1 1" : "1 1";
}

async function tick() {
  if (state !== "walk" || !bounds || moving) return;
  moving = true;
  x += dir * SPEED;
  if (x <= bounds.left)  { x = bounds.left;  dir = 1; }
  if (x >= bounds.right) { x = bounds.right; dir = -1; }
  updateFlip();
  await appWindow.setPosition(new PhysicalPosition(Math.round(x), Math.round(y)));
  moving = false;
}

// ================= УБОРКА ФАЙЛОВ (этап 4) =================
const queue = [];          // файлы, ждущие своей очереди
const handled = new Set(); // чтобы не обрабатывать один файл дважды
let processing = false;
let scanTimer = null;

// Просим Rust показать файлы на рабочем столе и ставим новые в очередь
async function scan() {
  try {
    const files = await invoke("scan_desktop");
    for (const f of files) {
      if (handled.has(f.path)) continue;
      handled.add(f.path);
      queue.push(f);
    }
    processQueue();
  } catch (e) {
    console.error("scan error:", e);
  }
}

// Rust шлёт много событий подряд, поэтому ждём тишины 3.5 секунды
function scheduleScan() {
  clearTimeout(scanTimer);
  scanTimer = setTimeout(scan, 3500);
}

async function processQueue() {
  if (processing) return;
  processing = true;
  while (queue.length > 0) {
    while (busy) await sleep(200); // ждём, пока питомец освободится
    await handleFile(queue.shift());
  }
  processing = false;
}

async function handleFile(file) {
  busy = true;
  lastActive = Date.now();
  setState("work");
  item.textContent = ICONS[file.category] ?? "📦";
  say(`Нашёл: ${shorten(file.name)}`);
  await sleep(1800);

  try {
    await invoke("move_file", { path: file.path, dryRun: DRY_RUN });
    say(`${DRY_RUN ? "Убрал бы" : "Убрал"} в ${file.category}`);
    if (!DRY_RUN) handled.delete(file.path); // файл ушёл, имя снова свободно
  } catch (e) {
    console.error("move error:", e);
    say("Не получилось 😿");
  }

  await sleep(1800);
  item.textContent = "";
  say("");
  busy = false;
  setState("idle");
}

// Правый клик по питомцу = отменить последнее перемещение
async function undo() {
  if (busy) return;
  busy = true;
  try {
    const restored = await invoke("undo_last");
    if (restored) {
      handled.add(restored); // чтобы питомец не убрал файл снова
      setState("happy");
      say("Вернул обратно");
    } else {
      say("Отменять нечего");
    }
  } catch (e) {
    console.error("undo error:", e);
    say("Не вышло отменить");
  }
  await sleep(2000);
  say("");
  busy = false;
  setState("idle");
}

pet.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  undo();
});

listen("desktop-changed", scheduleScan);

// ================= ПОВЕДЕНИЕ =================
pet.addEventListener("mouseenter", () => {
  lastActive = Date.now();
  if (busy) return;
  busy = true;
  setState("happy");
  setTimeout(() => { busy = false; setState("idle"); }, 1500);
});

setInterval(() => {
  if (busy) return;
  if (Date.now() - lastActive > 100000) { setState("sleep"); return; }
  setState(Math.random() < 0.5 ? "idle" : "walk");
}, 3000);

// ================= ЗАПУСК =================
(async () => {
  await initBounds();
  await syncPosition();
  y = bounds.bottom;
  x = Math.min(Math.max(x, bounds.left), bounds.right);
  await appWindow.setPosition(new PhysicalPosition(x, y));
  setState("idle");
  setInterval(tick, 33);
  scan(); // проверяем рабочий стол сразу при старте
})();