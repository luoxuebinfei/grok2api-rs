async function loadPublicHeader() {
  const container = document.getElementById('app-header');
  if (!container) return;
  try {
    const res = await fetch('/static/common/public-header.html?v=1');
    if (!res.ok) return;
    container.innerHTML = await res.text();
    const logoutBtn = container.querySelector('#public-logout-btn');
    if (logoutBtn) {
      logoutBtn.classList.add('hidden');
      try {
        const verify = await fetch('/v1/public/verify', { method: 'GET' });
        if (verify.status === 401) {
          logoutBtn.classList.remove('hidden');
        }
      } catch (e) { /* ignore */ }
    }
    const path = window.location.pathname;
    const links = container.querySelectorAll('a[data-nav]');
    links.forEach((link) => {
      const target = link.getAttribute('data-nav') || '';
      if (target && path.startsWith(target)) {
        link.classList.add('active');
      }
    });
  } catch (e) { /* ignore */ }
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', loadPublicHeader);
} else {
  loadPublicHeader();
}
