/**
 * 切换偏好分类时回到页首，新分类从第一个分组开始阅读。
 */
export function resetContentScroll(container: HTMLDivElement | null) {
  if (!container) return;

  container.scrollTo({
    behavior: "auto",
    top: 0,
  });
}

/**
 * 搜索跳转后把目标设置项滚到视野中，方便用户确认定位结果。
 */
export function scrollHighlightedSetting(
  container: HTMLDivElement | null,
  settingId: string,
  reduceMotion: boolean,
) {
  if (!container) return;

  const target = container.querySelector<HTMLElement>(
    `[data-preference-setting-id="${settingId}"]`,
  );
  if (!target) return;

  target.scrollIntoView({
    behavior: reduceMotion ? "auto" : "smooth",
    block: "center",
  });
}
