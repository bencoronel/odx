/** A modal is open: a confirmation (alertdialog) or a Radix dialog such as the command palette. Global shortcuts wait
 * behind it. Panels that only label themselves role="dialog" (filter popovers, the Terminal's value inspector) are not modal. */
export const modalOpen = () => !!document.querySelector('[role="alertdialog"], [role="dialog"][data-state="open"]');
