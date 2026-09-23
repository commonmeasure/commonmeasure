/* Resolve before paint. Storage may be unavailable in a restricted browser. */
(() => {
  const key = 'commonmeasure-theme';
  const root = document.documentElement;
  const normalise = value => value === 'dark' ? 'dark' : 'light';
  const apply = value => {
    root.dataset.theme = normalise(value);
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      const dark = root.dataset.theme === 'dark';
      const label = button.querySelector('.theme-label') || button;
      label.textContent = dark ? 'Light mode' : 'Dark mode';
      button.title = dark ? 'Switch to light mode' : 'Switch to dark mode';
      button.setAttribute('aria-label', dark ? 'Switch to light mode' : 'Switch to dark mode');
      button.dataset.tooltip = button.getAttribute('aria-label');
      button.hidden = false;
    });
  };
  const sidebarKey = 'commonmeasure-sidebar';
  const applySidebar = value => {
    const collapsed = value === 'collapsed';
    root.dataset.sidebar = collapsed ? 'collapsed' : 'expanded';
    document.querySelectorAll('[data-sidebar-toggle]').forEach(button => {
      button.setAttribute('aria-expanded', String(!collapsed));
      button.setAttribute('aria-label', collapsed ? 'Expand sidebar' : 'Collapse sidebar');
      button.title = collapsed ? 'Expand sidebar' : 'Collapse sidebar';
      button.dataset.tooltip = button.getAttribute('aria-label');
      button.hidden = false;
    });
  };
  try { apply(localStorage.getItem(key)); } catch { apply('light'); }
  try { applySidebar(localStorage.getItem(sidebarKey)); } catch { applySidebar('expanded'); }
  document.addEventListener('DOMContentLoaded', () => {
    apply(root.dataset.theme);
    applySidebar(root.dataset.sidebar);
    const side = document.querySelector('.side');
    const tooltip = document.createElement('span');
    tooltip.className = 'rail-tooltip';
    tooltip.setAttribute('aria-hidden', 'true');
    tooltip.hidden = true;
    document.body.append(tooltip);
    const hideTooltip = () => { tooltip.hidden = true; };
    const showTooltip = event => {
      const control = event.target.closest('[data-tooltip]');
      if (root.dataset.sidebar !== 'collapsed' || !control) return hideTooltip();
      tooltip.textContent = control.dataset.tooltip;
      tooltip.style.top = `${Math.min(innerHeight - 36, Math.max(8, control.getBoundingClientRect().top))}px`;
      tooltip.hidden = false;
    };
    side?.addEventListener('pointerover', showTooltip);
    side?.addEventListener('focusin', showTooltip);
    side?.addEventListener('pointerout', hideTooltip);
    side?.addEventListener('focusout', hideTooltip);
    side?.addEventListener('scroll', hideTooltip, true);
    side?.addEventListener('click', hideTooltip);
    const mobileMenu = document.querySelector('.mobile-nav');
    document.addEventListener('keydown', event => {
      if (event.key !== 'Escape') return;
      hideTooltip();
      if (mobileMenu?.open) {
        mobileMenu.open = false;
        mobileMenu.querySelector('summary').focus();
      }
    });
    document.addEventListener('pointerdown', event => {
      if (mobileMenu?.open && !side.contains(event.target)) mobileMenu.open = false;
    });
    document.querySelectorAll('[data-sidebar-toggle]').forEach(button => {
      button.addEventListener('click', () => {
        const value = root.dataset.sidebar === 'collapsed' ? 'expanded' : 'collapsed';
        applySidebar(value);
        try { localStorage.setItem(sidebarKey, value); } catch { /* Keep this page's choice. */ }
      });
    });
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      button.addEventListener('click', () => {
        const theme = root.dataset.theme === 'dark' ? 'light' : 'dark';
        apply(theme);
        try { localStorage.setItem(key, theme); } catch { /* Keep this page's choice. */ }
      });
    });
  });
  window.addEventListener('storage', event => {
    if (event.key === key || event.key === null) apply(event.newValue);
    if (event.key === sidebarKey || event.key === null) applySidebar(event.newValue);
  });
})();
