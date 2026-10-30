from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.region import Region


T = TypeVar("T", bound="SegmentationMask")


@_attrs_define
class SegmentationMask:
    """One selected object.

    Attributes:
        box (Region): A rectangle of an image, in its pixels: top-left origin, ends exclusive
            (it covers `x..x+w` by `y..y+h`). Named keys, never arrays: vendors
            disagree on axis order.
        png_base64 (str): A grayscale PNG the size of the image: white (255) where selected,
            black (0) elsewhere.
        score (Union[None, float]): The vendor's confidence, 0–1; null when it gives none.
    """

    box: "Region"
    png_base64: str
    score: Union[None, float]
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        box = self.box.to_dict()

        png_base64 = self.png_base64

        score: Union[None, float]
        score = self.score

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "box": box,
                "png_base64": png_base64,
                "score": score,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.region import Region

        d = src_dict.copy()
        box = Region.from_dict(d.pop("box"))

        png_base64 = d.pop("png_base64")

        def _parse_score(data: object) -> Union[None, float]:
            if data is None:
                return data
            return cast(Union[None, float], data)

        score = _parse_score(d.pop("score"))

        segmentation_mask = cls(
            box=box,
            png_base64=png_base64,
            score=score,
        )

        segmentation_mask.additional_properties = d
        return segmentation_mask

    @property
    def additional_keys(self) -> List[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
