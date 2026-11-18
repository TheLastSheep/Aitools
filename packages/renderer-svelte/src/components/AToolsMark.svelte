<script lang="ts">
  type Props = {
    size?: "small" | "badge" | "large";
    label?: string;
  };

  let { size = "badge", label = "" }: Props = $props();
</script>

<span
  class={`atools-mark ${size}`}
  aria-hidden={!label}
  aria-label={label || undefined}
  role={label ? "img" : undefined}
>
  <svg viewBox="0 0 128 128" aria-hidden="true" focusable="false">
    <path d="M25 99 55 22c1.7-4.4 3.8-6.5 8.1-6.5 4.1 0 6.4 2.1 8.1 6.5L103 99H82L75 80H53l-7 19H25Zm34-36h10l-5-14-5 14Z" />
  </svg>
</span>

<style>
  .atools-mark {
    --mark-size: 32px;
    --mark-radius: var(--radius-md);
    width: var(--mark-size);
    height: var(--mark-size);
    flex: 0 0 var(--mark-size);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    border-radius: var(--mark-radius);
    background: linear-gradient(135deg, #5b21b6 0%, #6d28d9 52%, #9333ea 100%);
    box-shadow:
      0 4px 12px -6px rgba(76, 29, 149, 0.55),
      inset 0 0 0 1px rgba(255, 255, 255, 0.18);
  }

  .atools-mark.small {
    --mark-size: 20px;
    --mark-radius: var(--radius-sm);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.18);
  }

  .atools-mark.large {
    --mark-size: 72px;
    --mark-radius: 16px;
  }

  svg {
    display: block;
    width: 72%;
    height: 72%;
    fill: #ffffff;
  }
</style>
