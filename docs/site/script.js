const platformCards = [...document.querySelectorAll('[data-platform-card]')];
const platformStatus = document.querySelector('[data-platform-status]');
const menuToggle = document.querySelector('[data-menu-toggle]');
const mobileMenu = document.querySelector('[data-mobile-menu]');
const viewTabs = [...document.querySelectorAll('[data-view]')];
const toast = document.querySelector('[data-toast]');
let toastTimer;

function detectPlatform() {
  const userAgent = navigator.userAgent;
  const isTablet = /Macintosh/i.test(userAgent) && navigator.maxTouchPoints > 0;
  if (/Android|iPhone|iPad|iPod|Mobile/i.test(userAgent) || isTablet) {
    return { key: null, message: 'LaterMd 是桌面应用，请在电脑上选择对应版本下载。' };
  }
  if (/Windows/i.test(userAgent)) {
    return { key: 'windows', message: '已为你突出显示 Windows 版，请确认系统为 Windows 11 或更新版本。' };
  }
  if (/Macintosh|Mac OS/i.test(userAgent)) {
    return { key: 'mac', message: '已为你突出显示 macOS 版，支持 Apple Silicon 与 Intel。' };
  }
  if (/Linux/i.test(userAgent) && !/CrOS/i.test(userAgent)) {
    return { key: 'linux', message: '已为你突出显示 Linux 版，请按发行版选择安装包。' };
  }
  return { key: null, message: '选择适合你的桌面系统，开始写作。' };
}

const platform = detectPlatform();
platformStatus.textContent = platform.message;
platformCards.forEach((card) => {
  card.classList.toggle('recommended', card.dataset.platformCard === platform.key);
});

function closeMenu(returnFocus = false) {
  mobileMenu.hidden = true;
  menuToggle.setAttribute('aria-expanded', 'false');
  menuToggle.setAttribute('aria-label', '打开菜单');
  if (returnFocus) menuToggle.focus();
}

menuToggle.addEventListener('click', () => {
  const isOpening = mobileMenu.hidden;
  mobileMenu.hidden = !isOpening;
  menuToggle.setAttribute('aria-expanded', String(isOpening));
  menuToggle.setAttribute('aria-label', isOpening ? '关闭菜单' : '打开菜单');
});

mobileMenu.querySelectorAll('a').forEach((link) => link.addEventListener('click', () => closeMenu()));
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !mobileMenu.hidden) closeMenu(true);
});
document.addEventListener('click', (event) => {
  if (!mobileMenu.hidden && !event.target.closest('.site-header')) closeMenu();
});
window.matchMedia('(min-width: 641px)').addEventListener('change', (event) => {
  if (event.matches) closeMenu();
});

function activateView(activeTab, moveFocus = false) {
  viewTabs.forEach((tab) => {
    const isSelected = tab === activeTab;
    tab.setAttribute('aria-selected', String(isSelected));
    tab.tabIndex = isSelected ? 0 : -1;
    document.getElementById(tab.getAttribute('aria-controls')).hidden = !isSelected;
  });
  if (moveFocus) activeTab.focus();
}

viewTabs.forEach((tab, index) => {
  tab.addEventListener('click', () => activateView(tab));
  tab.addEventListener('keydown', (event) => {
    let nextIndex = index;
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % viewTabs.length;
    else if (event.key === 'ArrowLeft') nextIndex = (index - 1 + viewTabs.length) % viewTabs.length;
    else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = viewTabs.length - 1;
    else return;
    event.preventDefault();
    activateView(viewTabs[nextIndex], true);
  });
});

function showToast(message) {
  clearTimeout(toastTimer);
  toast.textContent = message;
  toast.classList.add('visible');
  toastTimer = setTimeout(() => toast.classList.remove('visible'), 3500);
}

function copyFallback(text) {
  const previousFocus = document.activeElement;
  const helper = document.createElement('textarea');
  helper.value = text;
  helper.setAttribute('aria-label', '待复制的安装命令');
  helper.style.position = 'fixed';
  helper.style.opacity = '0';
  document.body.appendChild(helper);
  try {
    helper.focus();
    helper.select();
    if (!document.execCommand('copy')) throw new Error('Clipboard unavailable');
  } finally {
    helper.remove();
    previousFocus?.focus({ preventScroll: true });
  }
}

document.querySelectorAll('[data-copy-command]').forEach((button) => {
  button.addEventListener('click', async () => {
    const command = document.getElementById(button.dataset.copyCommand).textContent;
    button.disabled = true;
    try {
      if (navigator.clipboard?.writeText) {
        try {
          await navigator.clipboard.writeText(command);
        } catch {
          copyFallback(command);
        }
      } else {
        copyFallback(command);
      }
      showToast('Homebrew 安装命令已复制');
    } catch {
      showToast('复制未成功，请选中命令手动复制。');
    } finally {
      button.disabled = false;
    }
  });
});

document.querySelector('[data-year]').textContent = String(new Date().getFullYear());
