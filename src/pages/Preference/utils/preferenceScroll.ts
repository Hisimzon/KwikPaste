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

/** 与内容区 `p-6` 一致，跳转后分组卡片停在和首个分组相同的位置。 */
const SECTION_SCROLL_GAP = 24;
/** 分组顶部越过这条线（相对滚动容器顶部）即视为正在阅读该分组。 */
const SECTION_ACTIVE_LINE = 48;
/** 距离底部不足这段滚动距离时，激活线开始向视口底部移动。 */
const SECTION_SWEEP_DISTANCE = 240;

/**
 * 查找当前分类页里指定分组的卡片元素。
 */
function findSectionElement(container: HTMLElement, sectionId: string) {
  return container.querySelector<HTMLElement>(
    `[data-preference-section-id="${sectionId}"]`,
  );
}

/**
 * 页内目录点击后把分组卡片滚到内容区顶部。
 */
export function scrollToSection(
  container: HTMLDivElement | null,
  sectionId: string,
  reduceMotion: boolean,
) {
  if (!container) return;

  const target = findSectionElement(container, sectionId);
  if (!target) return;

  const offset =
    target.getBoundingClientRect().top - container.getBoundingClientRect().top;

  container.scrollTo({
    behavior: reduceMotion ? "auto" : "smooth",
    top: container.scrollTop + offset - SECTION_SCROLL_GAP,
  });
}

/**
 * 按滚动位置算出正在阅读的分组：取顶部已越过激活线的最后一个。
 * 末尾的短分组滚到底也到不了顶部，所以接近底部时激活线随剩余距离下移到视口底部，让它们依次高亮。
 * 只认传入的分组，切换分类时退场中的旧卡片不参与。
 */
export function resolveVisibleSectionId(
  container: HTMLDivElement,
  sectionIds: string[],
) {
  const sections = sectionIds.flatMap((sectionId) => {
    const element = findSectionElement(container, sectionId);

    return element ? [{ element, sectionId }] : [];
  });
  if (sections.length === 0) return null;

  const maxScrollTop = container.scrollHeight - container.clientHeight;
  const sweepDistance = Math.min(SECTION_SWEEP_DISTANCE, maxScrollTop);
  const remaining = maxScrollTop - container.scrollTop;
  const sweep =
    sweepDistance > 0 ? Math.max(0, 1 - remaining / sweepDistance) : 0;
  const activeLine =
    container.getBoundingClientRect().top +
    SECTION_ACTIVE_LINE +
    sweep * (container.clientHeight - SECTION_ACTIVE_LINE);
  let currentId = sections[0].sectionId;

  for (const { element, sectionId } of sections) {
    if (element.getBoundingClientRect().top > activeLine) break;

    currentId = sectionId;
  }

  return currentId;
}
