/* Appearance and sidebar preferences, resolved before paint. Light, Dark and
   System are explicit saved choices; a new visitor is light. Storage may be
   unavailable in a restricted browser, in which case a choice lasts for the
   page. */
(() => {
  const key = 'commonmeasure-theme';
  const sidebarKey = 'commonmeasure-sidebar';
  const root = document.documentElement;
  const choices = ['light', 'dark', 'system'];
  const normalise = value => (choices.includes(value) ? value : 'light');
  const osDark = window.matchMedia('(prefers-color-scheme: dark)');
  const effective = () => (root.dataset.theme === 'system' ? (osDark.matches ? 'dark' : 'light') : root.dataset.theme);
  const label = () => (effective() === 'dark' ? 'Switch to light mode' : 'Switch to dark mode');
  const apply = value => {
    root.dataset.theme = normalise(value);
    document.querySelectorAll('[data-theme-choice]').forEach(group => {
      group.hidden = false;
      group.querySelectorAll('button[data-theme-choice]').forEach(button => {
        button.setAttribute('aria-pressed', String(button.dataset.themeChoice === root.dataset.theme));
      });
    });
    // One button flips the effective theme: the guide's, in words, and the
    // collapsed rail's, as an icon with a tooltip.
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      if (button.dataset.themeToggle !== 'icon') button.textContent = effective() === 'dark' ? 'Light mode' : 'Dark mode';
      button.title = label();
      button.setAttribute('aria-label', label());
      button.dataset.tooltip = label();
      button.hidden = false;
    });
  };
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
  const remember = (name, value) => {
    try { localStorage.setItem(name, value); } catch { /* Keep this page's choice. */ }
  };
  try { apply(localStorage.getItem(key)); } catch { apply('light'); }
  try { applySidebar(localStorage.getItem(sidebarKey)); } catch { applySidebar('expanded'); }
  osDark.addEventListener('change', () => apply(root.dataset.theme));
  document.addEventListener('DOMContentLoaded', () => {
    apply(root.dataset.theme);
    applySidebar(root.dataset.sidebar);
    const side = document.querySelector('.shell-sidebar');
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
    const mobileMenu = document.querySelector('.shell-mobile-menu');
    document.addEventListener('keydown', event => {
      if (event.key !== 'Escape') return;
      hideTooltip();
      if (mobileMenu?.open) {
        mobileMenu.open = false;
        mobileMenu.querySelector('summary').focus();
      }
    });
    document.addEventListener('pointerdown', event => {
      if (mobileMenu?.open && !mobileMenu.contains(event.target)) mobileMenu.open = false;
    });
    document.querySelectorAll('[data-sidebar-toggle]').forEach(button => {
      button.addEventListener('click', () => {
        const value = root.dataset.sidebar === 'collapsed' ? 'expanded' : 'collapsed';
        applySidebar(value);
        remember(sidebarKey, value);
      });
    });
    document.querySelectorAll('button[data-theme-choice]').forEach(button => {
      button.addEventListener('click', () => {
        apply(button.dataset.themeChoice);
        remember(key, root.dataset.theme);
      });
    });
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      button.addEventListener('click', () => {
        apply(effective() === 'dark' ? 'light' : 'dark');
        remember(key, root.dataset.theme);
      });
    });
  });
  window.addEventListener('storage', event => {
    if (event.key === key || event.key === null) apply(event.newValue);
    if (event.key === sidebarKey || event.key === null) applySidebar(event.newValue);
  });
})();
