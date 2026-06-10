pub fn fill_text_script(title: &str, body: &str) -> String {
    let title = serde_json::to_string(title).unwrap_or_else(|_| "\"\"".to_string());
    let body = serde_json::to_string(body).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(() => {{
  const title = {title};
  const body = {body};
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }};
  const fire = (el, eventName) => el.dispatchEvent(new Event(eventName, {{ bubbles: true }}));
  const setNativeValue = (el, value) => {{
    const proto = Object.getPrototypeOf(el);
    const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
    if (descriptor && descriptor.set) descriptor.set.call(el, value);
    else el.value = value;
    el.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: value }}));
    fire(el, 'change');
  }};
  const titleEl = Array.from(document.querySelectorAll('textarea[placeholder*="标题"], input[placeholder*="标题"]'))
    .filter(visible)[0];
  if (titleEl) setNativeValue(titleEl, title);

  const editors = Array.from(document.querySelectorAll('.public-DraftEditor-content[contenteditable=true], [contenteditable=true]'))
    .filter(visible)
    .filter(el => !el.closest('[placeholder*="标题"]'));
  const bodyEl = editors[0];
  if (bodyEl) {{
    bodyEl.focus();
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(bodyEl);
    range.collapse(true);
    selection.removeAllRanges();
    selection.addRange(range);
    document.execCommand('insertText', false, body);
    bodyEl.dispatchEvent(new InputEvent('input', {{ bubbles: true, inputType: 'insertText', data: body }}));
    fire(bodyEl, 'change');
    bodyEl.blur();
  }}

  return {{
    message: `知乎草稿已填充：标题${{titleEl ? '成功' : '未找到'}}，正文${{bodyEl ? '成功' : '未找到'}}。`
  }};
}})()
"#
    )
}

pub fn main_publish_center_script() -> &'static str {
    r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const disabled = (el) => {
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  };
  const candidates = Array.from(document.querySelectorAll('button, [role=button]'))
    .filter(visible)
    .filter(el => !disabled(el))
    .map(el => {
      const label = (el.innerText || el.textContent || '').trim();
      const rect = el.getBoundingClientRect();
      const cls = String(el.className || '').toLowerCase();
      return { label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height, cls };
    })
    .filter(item => item.label === '发布' && item.area >= 500 && item.area <= 12000 && item.y > window.innerHeight - 90)
    .sort((a, b) => b.x - a.x || b.y - a.y || a.area - b.area);
  return candidates[0] || null;
})()
"#
}

pub fn confirm_publish_center_script() -> &'static str {
    r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const disabled = (el) => {
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  };
  const roots = Array.from(document.querySelectorAll('[role=dialog], .Modal, .Dialog, .modal, .popover'))
    .filter(visible);
  const searchRoots = roots.length ? roots : [document.body];
  const candidates = searchRoots.flatMap(root => Array.from(root.querySelectorAll('button, [role=button]')))
    .filter(visible)
    .filter(el => !disabled(el))
    .map(el => {
      const label = (el.innerText || el.textContent || '').replace(/\s+/g, '').trim();
      const rect = el.getBoundingClientRect();
      return { label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => ['发布', '确认发布', '发布文章'].includes(item.label) && item.area >= 500 && item.area <= 20000)
    .sort((a, b) => b.y - a.y || b.x - a.x || a.area - b.area);
  return candidates[0] || null;
})()
"#
}
