// Bundle discount, fixed build. Must pass every promochaos rule.
//
// Lines tagged `bundle` (never gift cards): when the cart holds 2 or more
// bundle items, each bundle line gets 5 percent off per extra item, at most
// 50 percent. Only the BUNDLE10 code is accepted.

const STEP_PERCENT = 5;
const MAX_PERCENT = 50;
const ACCEPTED_CODES = ['BUNDLE10'];

function isBundleLine(line) {
  return (
    line.merchandise.__typename === 'ProductVariant' &&
    line.merchandise.product.bundle &&
    !line.merchandise.product.isGiftCard
  );
}

/**
 * @param {object} input The function input from cart_lines_discounts_generate_run.graphql.
 * @returns {{operations: object[]}}
 */
export function cartLinesDiscountsGenerateRun(input) {
  const operations = [];

  // Exact match, so the accepted code is always one of ACCEPTED_CODES.
  const accepted = input.enteredDiscountCodes.filter((entered) =>
    ACCEPTED_CODES.includes(entered.code),
  );
  if (accepted.length > 0) {
    operations.push({
      enteredDiscountCodesAccept: {
        codes: accepted.map((entered) => ({code: entered.code})),
      },
    });
  }

  if (!input.discount.discountClasses.includes('PRODUCT')) {
    return {operations};
  }

  const bundleLines = input.cart.lines.filter(isBundleLine);
  const bundleItems = bundleLines.reduce((count, line) => count + line.quantity, 0);

  if (bundleItems < 2) {
    return {operations};
  }

  const percentage = Math.min(STEP_PERCENT * (bundleItems - 1), MAX_PERCENT);

  operations.push({
    productDiscountsAdd: {
      candidates: [
        {
          message: `Bundle ${percentage}% off`,
          targets: bundleLines.map((line) => ({cartLine: {id: line.id}})),
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
