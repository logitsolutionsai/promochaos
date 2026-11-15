// Bundle discount, default build with three seeded bugs for promochaos.
// The fixed version is cart_lines_discounts_generate_run.fixed.js.
//
// Lines tagged `bundle`: when the cart holds 2 or more bundle items, each
// bundle line gets 5 percent off per extra item.

const STEP_PERCENT = 5;

/**
 * @param {object} input The function input from cart_lines_discounts_generate_run.graphql.
 * @returns {{operations: object[]}}
 */
export function cartLinesDiscountsGenerateRun(input) {
  const operations = [];

  // Seeded bug 3: every entered code is accepted, not only BUNDLE10.
  if (input.enteredDiscountCodes.length > 0) {
    operations.push({
      enteredDiscountCodesAccept: {
        codes: input.enteredDiscountCodes.map((entered) => ({code: entered.code})),
      },
    });
  }

  if (!input.discount.discountClasses.includes('PRODUCT')) {
    return {operations};
  }

  const bundleItems = input.cart.lines
    .filter(
      (line) =>
        line.merchandise.__typename === 'ProductVariant' &&
        line.merchandise.product.bundle,
    )
    .reduce((count, line) => count + line.quantity, 0);

  if (bundleItems < 2) {
    return {operations};
  }

  // Seeded bug 2: the percentage has no cap, so a cart with 22 or more bundle
  // items emits a percentage over 100.
  const percentage = STEP_PERCENT * (bundleItems - 1);

  // Seeded bug 1: every line is a target, gift cards included.
  const targets = input.cart.lines.map((line) => ({cartLine: {id: line.id}}));

  operations.push({
    productDiscountsAdd: {
      candidates: [
        {
          message: `Bundle ${percentage}% off`,
          targets,
          value: {
            percentage: {
              value: percentage,
            },
          },
        },
      ],
      selectionStrategy: 'ALL',
    },
  });

  return {operations};
}
