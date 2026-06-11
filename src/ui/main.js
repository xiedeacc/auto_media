const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event?.listen;

const els = {
  message: document.querySelector("#message"),
  state: document.querySelector("#state"),
  lastTick: document.querySelector("#last-tick"),
  recentCheck: document.querySelector("#recent-check"),
  nextWakeup: document.querySelector("#next-wakeup"),
  manualPost: document.querySelector("#manual-post"),
  runNow: document.querySelector("#run-now"),
  pauseToggle: document.querySelector("#pause-toggle"),
  logsButton: document.querySelector("#logs-button"),
  clearRecords: document.querySelector("#clear-records"),
  autostart: document.querySelector("#autostart"),
  records: document.querySelector("#records"),
  manualModal: document.querySelector("#manual-modal"),
  logsModal: document.querySelector("#logs-modal"),
  imagePreviewModal: document.querySelector("#image-preview-modal"),
  progressModal: document.querySelector("#progress-modal"),
  logsContent: document.querySelector("#logs-content"),
  previewImage: document.querySelector("#preview-image"),
  previewCaption: document.querySelector("#preview-caption"),
  progressTitle: document.querySelector("#progress-title"),
  progressList: document.querySelector("#progress-list"),
  closeProgress: document.querySelector("#close-progress"),
  selectImages: document.querySelector("#select-images"),
  imageCount: document.querySelector("#image-count"),
  imageList: document.querySelector("#image-list"),
  manualTitle: document.querySelector("#manual-title"),
  manualText: document.querySelector("#manual-text"),
  manualTags: document.querySelector("#manual-tags"),
  submitManual: document.querySelector("#submit-manual"),
  manualTemplate: document.querySelector("#manual-template"),
  manualPlatforms: document.querySelectorAll('input[name="manual-platform"]'),
  platformStatuses: document.querySelectorAll("[data-platform-status]"),
  loginButtons: document.querySelectorAll("[data-login]"),
};

let paused = false;
let busy = false;
let manualImages = [];
let lastMessage = "";
let defaultTags = [];
let publishTitlePattern = "";
let lastTradeTitle = "";
let lastTradeTags = "";
let manualProgressItems = [];

function fmt(value) {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

async function call(command, args = {}) {
  return await invoke(command, args);
}

async function refresh() {
  try {
    const data = await call("get_status");
    const status = data.status;
    paused = status.paused;
    lastMessage = status.last_message || "";
    els.message.textContent = "";
    els.state.textContent = paused ? "stopped" : status.state;
    els.lastTick.textContent = status.last_tick ? "最近状态已更新" : "-";
    els.recentCheck.textContent = fmt(status.last_tick);
    els.nextWakeup.textContent = fmt(status.next_wakeup);
    els.pauseToggle.textContent = paused ? "启动" : "停止";
    els.autostart.checked = Boolean(data.autostart_enabled);
    defaultTags = data.publish_tags || [];
    publishTitlePattern = data.publish_title_pattern || "";
    if (!els.manualModal.classList.contains("hidden")) {
      applyManualTemplate();
    }
    renderPlatformSessions(data.platform_sessions || []);
    renderRecords(status.recent_platform_statuses || []);
  } catch (error) {
    showError(error);
  }
}

function compactMessage(status) {
  const message = status.last_message || "";
  if (!message || message === "-") return "等待任务";
  if (status.state === "manual_publish") return "手动发文已处理，详细内容见日志";
  if (status.state === "error") return "任务异常，点击日志查看详情";
  if (message.length > 42) return `${message.slice(0, 42)}...`;
  return message;
}

function showError(error) {
    lastMessage = error?.message || String(error);
    els.message.textContent = "";
}

function openManualModal() {
  applyManualTemplate();
  els.manualModal.classList.remove("hidden");
  els.manualTitle.focus();
}

function closeManualModal() {
  els.manualModal.classList.add("hidden");
}

async function openLogsModal() {
  els.logsModal.classList.remove("hidden");
  els.logsContent.textContent = "加载中";
  try {
    const logs = await call("get_logs", { lines: 240 });
    const manual = lastMessage ? `最近消息:\n${lastMessage}\n\n` : "";
    els.logsContent.textContent = `${manual}${logs || "暂无日志"}`;
  } catch (error) {
    els.logsContent.textContent = error.message;
  }
}

function closeLogsModal() {
  els.logsModal.classList.add("hidden");
}

function closeImagePreview() {
  els.imagePreviewModal.classList.add("hidden");
  els.previewImage.removeAttribute("src");
  els.previewCaption.textContent = "";
}

function platformLabel(platform) {
  if (platform === "xhs") return "小红书";
  if (platform === "zhihu") return "知乎";
  if (platform === "twitter") return "Twitter/X";
  return platform || "-";
}

function progressStatusText(status) {
  if (status === "pending") return "等待";
  if (status === "publishing") return "发送中";
  if (status === "success") return "成功";
  if (status === "failed") return "失败";
  if (status === "done") return "完成";
  return status || "-";
}

function openProgressModal(platforms) {
  manualProgressItems = platforms.map((platform) => ({
    platform,
    status: "pending",
    message: "等待发送",
  }));
  els.progressTitle.textContent = "发送进度";
  els.closeProgress.disabled = false;
  renderProgress();
  els.progressModal.classList.remove("hidden");
}

function closeProgressModal() {
  els.progressModal.classList.add("hidden");
}

function renderProgress() {
  els.progressList.innerHTML = manualProgressItems
    .map(
      (item) => `<div class="progress-row">
        <span>${escapeHtml(platformLabel(item.platform))}</span>
        <strong class="progress-status ${escapeHtml(item.status)}">${escapeHtml(progressStatusText(item.status))}</strong>
        <small>${escapeHtml(item.message)}</small>
      </div>`,
    )
    .join("");
}

function updateManualProgress(payload) {
  if (!payload) return;

  if (!payload.platform) {
    if (payload.status === "start") {
      els.progressTitle.textContent = payload.message || "发送进度";
    } else if (payload.status === "done") {
      els.progressTitle.textContent = "发送完成";
    }
    return;
  }

  const index = manualProgressItems.findIndex(
    (item) => item.platform === payload.platform,
  );
  const item = {
    platform: payload.platform,
    status: payload.status || "publishing",
    message: payload.message || "",
  };
  if (index >= 0) {
    manualProgressItems[index] = item;
  } else {
    manualProgressItems.push(item);
  }
  renderProgress();
}

function selectedManualPlatforms() {
  return Array.from(els.manualPlatforms)
    .filter((input) => input.checked)
    .map((input) => input.value);
}

function selectedManualTemplate() {
  return els.manualTemplate?.value || "trade";
}

function yesterdayParts() {
  const date = new Date();
  date.setDate(date.getDate() - 1);
  const year = String(date.getFullYear());
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return { year, month, day };
}

function renderTitlePattern(pattern) {
  const { year, month, day } = yesterdayParts();
  return (pattern || "")
    .replaceAll("{YYYYMMDD}", `${year}${month}${day}`)
    .replaceAll("{YYYY-MM-DD}", `${year}-${month}-${day}`);
}

function applyManualTemplate() {
  const template = selectedManualTemplate();
  const tradeTitle = renderTitlePattern(publishTitlePattern);
  const tradeTags = defaultTags.join(" ");

  if (template === "trade") {
    if (!els.manualTitle.value.trim() || els.manualTitle.value === lastTradeTitle) {
      els.manualTitle.value = tradeTitle;
    }
    if (!els.manualTags.value.trim() || els.manualTags.value === lastTradeTags) {
      els.manualTags.value = tradeTags;
    }
    lastTradeTitle = tradeTitle;
    lastTradeTags = tradeTags;
    return;
  }

  if (els.manualTitle.value === lastTradeTitle) {
    els.manualTitle.value = "";
  }
  if (els.manualTags.value === lastTradeTags) {
    els.manualTags.value = "";
  }
}

function renderManualImages() {
  els.imageCount.textContent = manualImages.length
    ? `已选择 ${manualImages.length} 张图片`
    : "未选择图片，可按 Ctrl+V 粘贴";
  els.imageList.innerHTML = manualImages
    .map(
      (path, index) => `<div class="image-row" role="button" tabindex="0" data-preview-image="${index}">
        <span>${escapeHtml(path)}</span>
        <button type="button" data-remove-image="${index}">移除</button>
      </div>`,
    )
    .join("");

  els.imageList.querySelectorAll("[data-remove-image]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      manualImages.splice(Number(button.dataset.removeImage), 1);
      renderManualImages();
    });
  });

  els.imageList.querySelectorAll("[data-preview-image]").forEach((row) => {
    row.addEventListener("click", () => {
      previewManualImage(Number(row.dataset.previewImage));
    });
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        previewManualImage(Number(row.dataset.previewImage));
      }
    });
  });
}

async function previewManualImage(index) {
  const path = manualImages[index];
  if (!path) return;
  els.imagePreviewModal.classList.remove("hidden");
  els.previewCaption.textContent = path;
  els.previewImage.removeAttribute("src");
  try {
    els.previewImage.src = await call("read_image_preview", { path });
  } catch (error) {
    closeImagePreview();
    showError(error);
    window.alert(error?.message || String(error));
  }
}

async function saveClipboardImage(file, index) {
  const buffer = await file.arrayBuffer();
  const bytes = Array.from(new Uint8Array(buffer));
  const extension = mimeToExtension(file.type);
  const fileName = file.name || `clipboard-${index}.${extension}`;
  return await call("save_pasted_image", { fileName, bytes });
}

function mimeToExtension(mime) {
  if (mime === "image/jpeg") return "jpg";
  if (mime === "image/webp") return "webp";
  return "png";
}

async function handlePaste(event) {
  if (els.manualModal.classList.contains("hidden")) {
    return;
  }

  const items = Array.from(event.clipboardData?.items || []);
  const imageItems = items.filter((item) => item.type.startsWith("image/"));
  if (!imageItems.length) {
    return;
  }

  event.preventDefault();
  try {
    const saved = [];
    for (const [index, item] of imageItems.entries()) {
      const file = item.getAsFile();
      if (file) {
        saved.push(await saveClipboardImage(file, index + 1));
      }
    }
    manualImages = Array.from(new Set([...manualImages, ...saved]));
    renderManualImages();
  } catch (error) {
    showError(error);
  }
}

function renderPlatformSessions(sessions) {
  const byPlatform = new Map(sessions.map((session) => [session.platform, session]));
  els.platformStatuses.forEach((node) => {
    const platform = node.dataset.platformStatus;
    const session = byPlatform.get(platform);
    const label = session?.label || "未启用";
    node.textContent = label;
    node.className = `platform-status ${statusClass(label)}`;
  });
  els.loginButtons.forEach((button) => {
    const session = byPlatform.get(button.dataset.login);
    button.textContent = session?.label === "已登录" ? "重新登录" : "登录";
  });
}

function statusClass(label) {
  if (label === "已登录") return "ok";
  if (label === "需确认") return "pending";
  if (label === "未登录") return "missing";
  return "unknown";
}

function renderRecords(records) {
  if (!records.length) {
    els.records.innerHTML = '<tr><td colspan="5">暂无任务</td></tr>';
    return;
  }

  els.records.innerHTML = records
    .map((record) => {
      const message = record.last_error ? "有错误，点击日志查看" : record.remote_url || "";
      return `<tr>
        <td>${escapeHtml(record.platform)}</td>
        <td>${escapeHtml(statusText(record.status))}</td>
        <td>${record.attempt_count}</td>
        <td>${escapeHtml(fmt(record.updated_at))}</td>
        <td>${escapeHtml(message)}</td>
      </tr>`;
    })
    .join("");
}

function statusText(status) {
  if (status === "success") return "发送成功";
  if (status === "failed") return "发送失败";
  if (status === "publishing") return "发送中";
  if (status === "auth_required") return "需要登录";
  if (status === "skipped_duplicate") return "已跳过";
  return status || "-";
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

els.runNow.addEventListener("click", async () => {
  if (busy) return;
  busy = true;
  els.runNow.disabled = true;
  try {
    await call("run_now");
    await refresh();
  } catch (error) {
    showError(error);
  } finally {
    busy = false;
    els.runNow.disabled = false;
  }
});

els.manualPost.addEventListener("click", openManualModal);
els.logsButton.addEventListener("click", openLogsModal);
els.closeProgress.addEventListener("click", closeProgressModal);

els.clearRecords.addEventListener("click", async () => {
  if (busy) return;
  const confirmed = window.confirm("确定清空所有最近任务记录？");
  if (!confirmed) return;
  busy = true;
  els.clearRecords.disabled = true;
  try {
    await call("clear_records");
    await refresh();
  } catch (error) {
    showError(error);
  } finally {
    busy = false;
    els.clearRecords.disabled = false;
  }
});

document.querySelectorAll("[data-close-manual]").forEach((node) => {
  node.addEventListener("click", closeManualModal);
});

document.querySelectorAll("[data-close-logs]").forEach((node) => {
  node.addEventListener("click", closeLogsModal);
});

document.querySelectorAll("[data-close-preview]").forEach((node) => {
  node.addEventListener("click", closeImagePreview);
});

window.addEventListener("paste", handlePaste);

els.manualTemplate.addEventListener("change", applyManualTemplate);

els.selectImages.addEventListener("click", async () => {
  try {
    const selected = await call("select_images");
    manualImages = Array.from(new Set([...manualImages, ...selected]));
    renderManualImages();
  } catch (error) {
    showError(error);
  }
});

els.submitManual.addEventListener("click", async () => {
  if (busy) return;
  if (!manualImages.length) {
    lastMessage = "请选择至少一张图片";
    window.alert(lastMessage);
    return;
  }
  const platforms = selectedManualPlatforms();
  if (!platforms.length) {
    lastMessage = "请选择至少一个发布平台";
    window.alert(lastMessage);
    return;
  }
  const effectiveText = `${els.manualText.value}\n${els.manualTags.value}`.trim();
  if (platforms.includes("zhihu") && effectiveText.length < 9) {
    lastMessage = "知乎正文至少需要 9 个字";
    window.alert(lastMessage);
    return;
  }

  busy = true;
  els.submitManual.disabled = true;
  openProgressModal(platforms);
  try {
    const message = await call("manual_publish", {
      title: els.manualTitle.value,
      text: els.manualText.value,
      tags: els.manualTags.value,
      imagePaths: manualImages,
      platforms,
    });
    lastMessage = message;
    els.progressTitle.textContent = "发送完成";
    closeManualModal();
    await refresh();
  } catch (error) {
    els.progressTitle.textContent = "发送失败";
    if (!manualProgressItems.some((item) => item.status === "failed")) {
      manualProgressItems.push({
        platform: "manual",
        status: "failed",
        message: error?.message || String(error),
      });
      renderProgress();
    }
    showError(error);
  } finally {
    busy = false;
    els.submitManual.disabled = false;
  }
});

els.pauseToggle.addEventListener("click", async () => {
  try {
    await call("set_paused", { paused: !paused });
    await refresh();
  } catch (error) {
    showError(error);
  }
});

els.autostart.addEventListener("change", async () => {
  try {
    await call("set_autostart", { enabled: els.autostart.checked });
    await refresh();
  } catch (error) {
    showError(error);
    els.autostart.checked = !els.autostart.checked;
  }
});

els.loginButtons.forEach((button) => {
  button.addEventListener("click", async () => {
    try {
      await call("login_platform", { platform: button.dataset.login });
      await refresh();
    } catch (error) {
      showError(error);
    }
  });
});

document.querySelectorAll("[data-open]").forEach((button) => {
  button.addEventListener("click", async () => {
    try {
      await call("open_dir", { kind: button.dataset.open });
    } catch (error) {
      showError(error);
    }
  });
});

refresh();
setInterval(refresh, 5000);

if (listen) {
  listen("manual_publish_progress", (event) => {
    updateManualProgress(event.payload);
  });
}
