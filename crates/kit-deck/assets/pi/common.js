(function () {
  let websocket = null;
  let uuid = null;
  let settings = {};

  function applySettingsToDom() {
    document.querySelectorAll('[data-setting]').forEach((el) => {
      const key = el.dataset.setting;
      if (settings[key] !== undefined && settings[key] !== null) {
        el.value = String(settings[key]);
      } else if (el.dataset.default !== undefined) {
        el.value = el.dataset.default;
      }
    });
  }

  function saveSettingsFromDom() {
    const next = { ...settings };
    document.querySelectorAll('[data-setting]').forEach((el) => {
      const key = el.dataset.setting;
      if (el.dataset.type === 'number') {
        const parsed = parseInt(el.value, 10);
        if (!Number.isNaN(parsed)) next[key] = parsed;
      } else {
        const val = el.value.trim();
        if (val) next[key] = val;
        else delete next[key];
      }
    });
    settings = next;
    if (websocket && websocket.readyState === WebSocket.OPEN) {
      websocket.send(JSON.stringify({
        event: 'setSettings',
        context: uuid,
        payload: settings,
      }));
    }
  }

  window.connectElgatoStreamDeckSocket = function (inPort, inUUID, inRegisterEvent, _inInfo, inActionInfo) {
    uuid = inUUID;
    try {
      const info = JSON.parse(inActionInfo || '{}');
      settings = (info.payload && info.payload.settings) || {};
    } catch (_) {
      settings = {};
    }
    applySettingsToDom();
    document.querySelectorAll('[data-setting]').forEach((el) => {
      el.addEventListener('change', saveSettingsFromDom);
      el.addEventListener('input', saveSettingsFromDom);
    });
    websocket = new WebSocket(`ws://127.0.0.1:${inPort}`);
    websocket.onopen = function () {
      websocket.send(JSON.stringify({ event: inRegisterEvent, uuid }));
    };
    websocket.onmessage = function (evt) {
      try {
        const msg = JSON.parse(evt.data);
        if (msg.event === 'didReceiveSettings' && msg.payload && msg.payload.settings) {
          settings = msg.payload.settings;
          applySettingsToDom();
        }
      } catch (_) {}
    };
  };
})();
