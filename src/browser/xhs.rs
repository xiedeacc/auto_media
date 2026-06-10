pub fn image_tab_center_script() -> &'static str {
    r#"
(() => {
  const visible = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && rect.x >= 0 && rect.y >= 0 && style.visibility !== 'hidden' && style.display !== 'none';
  };
  const tabs = Array.from(document.querySelectorAll('.creator-tab'))
    .filter(visible)
    .map(el => {
      const label = (el.innerText || el.textContent || '').trim();
      const rect = el.getBoundingClientRect();
      return { label, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, area: rect.width * rect.height };
    })
    .filter(item => item.label === '上传图文' && item.area >= 1000)
    .sort((a, b) => a.area - b.area);
  return tabs[0] || null;
})()
"#
}

pub fn evaluate_publish_script(text: &str) -> String {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"
(async () => {{
  const wanted = {text};
  const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));
  const visible = (el) => {{
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    return rect.width > 0 &&
      rect.height > 0 &&
      style.visibility !== 'hidden' &&
      style.display !== 'none';
  }};
  const disabled = (el) => {{
    const aria = el.getAttribute('aria-disabled');
    const cls = String(el.className || '').toLowerCase();
    return el.disabled || aria === 'true' || cls.includes('disabled');
  }};
  const textOf = (el) => (el.innerText || el.textContent || '').replace(/\s+/g, '').trim();
  const clickable = (el) => el.closest('button, [role=button]') || el;
  const fireEvaluateEvents = async (el) => {{
    el.focus?.();
    const rect = el.getBoundingClientRect();
    const x = rect.x + rect.width / 2;
    const y = rect.y + rect.height / 2;
    const mouseOptions = {{ bubbles: true, cancelable: true, composed: true, view: window, clientX: x, clientY: y }};
    const pointerOptions = {{ ...mouseOptions, pointerId: 1, pointerType: 'mouse', isPrimary: true }};
    for (const type of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointerdown']) {{
      el.dispatchEvent(new PointerEvent(type, pointerOptions));
    }}
    el.dispatchEvent(new MouseEvent('mousedown', mouseOptions));
    for (const type of ['pointerup', 'mouseup', 'click']) {{
      el.dispatchEvent(new MouseEvent(type, mouseOptions));
    }}
    el.click?.();
    for (const name of ['submit', 'publish', 'confirm', 'click-submit']) {{
      el.dispatchEvent(new CustomEvent(name, {{ bubbles: true, cancelable: true, composed: true, detail: {{ source: 'auto_media' }} }}));
    }}
    await sleep(300);
    return {{ x, y }};
  }};

  const publishHost = Array.from(document.querySelectorAll('xhs-publish-btn'))
    .filter(visible)
    .filter(el => (el.getAttribute('submit-text') || '').trim() === wanted)
    .filter(el => (el.getAttribute('submit-disabled') || '').trim() !== 'true')
    .sort((a, b) => {{
      const ar = a.getBoundingClientRect();
      const br = b.getBoundingClientRect();
      return br.y - ar.y || br.x - ar.x;
    }})[0];
  if (publishHost) {{
    const point = await fireEvaluateEvents(publishHost);
    return {{
      clicked: true,
      label: publishHost.getAttribute('submit-text') || wanted,
      x: point.x,
      y: point.y,
      selector: 'xhs-publish-btn[submit-text]'
    }};
  }}

  const candidates = Array.from(document.querySelectorAll('button, [role=button], div'))
    .filter(visible)
    .filter(el => !disabled(clickable(el)))
    .map(el => {{
      const target = clickable(el);
      const rect = target.getBoundingClientRect();
      return {{
        el: target,
        label: textOf(target),
        x: rect.x + rect.width / 2,
        y: rect.y + rect.height / 2,
        area: rect.width * rect.height
      }};
    }})
    .filter(item =>
      item.label === wanted &&
      item.area >= 300 &&
      item.area <= 60000
    )
    .sort((a, b) =>
      b.y - a.y ||
      b.x - a.x ||
      a.area - b.area
    );

  const item = candidates[0];
  if (!item) {{
    return {{ clicked: false, reason: 'not_found' }};
  }}
  const point = await fireEvaluateEvents(item.el);
  return {{
    clicked: true,
    label: item.label,
    x: point.x,
    y: point.y,
    selector: 'fallback-text'
  }};
}})()
"#
    )
}

pub fn publish_with_api_template_script(
    url: &str,
    method: &str,
    post_data: Option<&str>,
    content_type: Option<&str>,
) -> String {
    let url = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".to_string());
    let method = serde_json::to_string(method).unwrap_or_else(|_| "\"POST\"".to_string());
    let post_data = serde_json::to_string(&post_data).unwrap_or_else(|_| "null".to_string());
    let content_type = serde_json::to_string(&content_type).unwrap_or_else(|_| "null".to_string());
    format!(
        r#"
(async () => {{
  const url = {url};
  const method = {method};
  const postData = {post_data};
  const contentType = {content_type};
  const headers = {{}};
  if (contentType) headers['content-type'] = contentType;
  const response = await fetch(url, {{
    method,
    credentials: 'include',
    headers,
    body: postData || undefined
  }});
  const text = await response.text();
  return {{
    ok: response.ok,
    status: response.status,
    text: text.slice(0, 1000)
  }};
}})()
"#
    )
}
