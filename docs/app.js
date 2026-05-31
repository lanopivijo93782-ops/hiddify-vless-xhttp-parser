// Канонический URL подписки (импорт в Hiddify).
const SUB_URL =
  "https://raw.githubusercontent.com/lanopivijo93782-ops/hiddify-vless-xhttp-parser/main/sub/subscription.txt";

// Имя профиля, которое увидит пользователь в Hiddify.
const PROFILE_NAME = "🇷🇺 RU VLESS";

// Человеческие названия провайдеров для статистики.
const PROVIDER_NAMES = {
  vk: "VK · ВКонтакте",
  yandex: "YA · Яндекс",
  mts: "MTS · МТС",
  beeline: "Beeline · Билайн",
  megafon: "MegaFon · МегаФон",
  rostelecom: "Rostelecom · Ростелеком",
  tele2: "Tele2 · Теле2",
  ertelecom: "ER-Telecom · ЭР-Телеком",
  ttk: "TTK · ТТК",
};

function hiddifyImportLink(url) {
  // Схема Hiddify: hiddify://import/<URL>#<имя>
  return "hiddify://import/" + url + "#" + encodeURIComponent(PROFILE_NAME);
}

function qrSrc(url) {
  return (
    "https://api.qrserver.com/v1/create-qr-code/?size=220x220&margin=0&data=" +
    encodeURIComponent(url)
  );
}

function showToast(text) {
  const t = document.getElementById("toast");
  t.textContent = text;
  t.classList.add("show");
  setTimeout(() => t.classList.remove("show"), 1800);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    showToast("Ссылка скопирована");
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
    showToast("Ссылка скопирована");
  }
}

function initSubscription() {
  document.getElementById("url-box").textContent = SUB_URL;
  document.getElementById("import-btn").href = hiddifyImportLink(SUB_URL);
  document.getElementById("qr").src = qrSrc(SUB_URL);

  document
    .getElementById("copy-btn")
    .addEventListener("click", () => copyText(SUB_URL));
  document
    .getElementById("url-box")
    .addEventListener("click", () => copyText(SUB_URL));
}

async function loadStats() {
  try {
    const res = await fetch("sub/report.json?cb=" + Date.now());
    if (!res.ok) throw new Error("report.json " + res.status);
    const r = await res.json();

    document.getElementById("stat-total").textContent = r.total ?? "—";
    const byProv = r.by_provider || {};
    const provKeys = Object.keys(byProv);
    document.getElementById("stat-providers").textContent = provKeys.length || "—";
    document.getElementById("stat-updated").textContent = (r.generated_at || "—")
      .replace("T", " ")
      .replace("Z", "");

    const wrap = document.getElementById("providers");
    if (provKeys.length === 0) {
      wrap.innerHTML =
        '<span class="muted">Сейчас нет активных серверов на российских сетях. Обновление каждые 6 часов.</span>';
    } else {
      wrap.innerHTML = "";
      provKeys
        .sort((a, b) => byProv[b] - byProv[a])
        .forEach((k) => {
          const chip = document.createElement("span");
          chip.className = "provider-chip";
          chip.innerHTML =
            "🇷🇺 " +
            (PROVIDER_NAMES[k] || k) +
            ' <span class="cnt">' +
            byProv[k] +
            "</span>";
          wrap.appendChild(chip);
        });
    }
  } catch (e) {
    document.getElementById("providers").innerHTML =
      '<span class="muted">Не удалось загрузить статистику.</span>';
  }
}

async function loadServerList() {
  const list = document.getElementById("server-list");
  try {
    const res = await fetch("sub/vless.txt?cb=" + Date.now());
    if (!res.ok) throw new Error("vless.txt " + res.status);
    const text = await res.text();
    const names = text
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.startsWith("vless://") && l.includes("#"))
      .map((l) => {
        try {
          return decodeURIComponent(l.slice(l.indexOf("#") + 1));
        } catch {
          return l.slice(l.indexOf("#") + 1);
        }
      });

    document.getElementById("srv-count").textContent =
      names.length ? "(" + names.length + ")" : "";

    if (names.length === 0) {
      list.innerHTML =
        '<span class="muted">Список пуст. Серверы появятся после следующего обновления.</span>';
      return;
    }

    list.innerHTML = "";
    names.forEach((n) => {
      const row = document.createElement("div");
      row.className = "server-row";
      // Имя вида "🇷🇺:YA #01 · 35ms" — отделяем скорость после "·".
      const parts = n.split("·");
      const name = (parts[0] || n).trim();
      const ping = parts[1] ? parts[1].trim() : "";
      row.innerHTML =
        '<span class="name">' +
        name +
        '</span><span class="ping">' +
        ping +
        "</span>";
      list.appendChild(row);
    });
  } catch (e) {
    list.innerHTML =
      '<span class="muted">Не удалось загрузить список серверов.</span>';
  }
}

initSubscription();
loadStats();
loadServerList();
